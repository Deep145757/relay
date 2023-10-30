use serde::ser::SerializeMap;
use serde::Serialize;

use crate::aggregator;
use std::ops::Range;

use crate::bucket::Bucket;
use crate::{BucketValue, DistributionValue};

/// The fraction of [`AggregatorServiceConfig::max_flush_bytes`] at which buckets will be split. A value of
/// `2` means that all buckets smaller than half of max_flush_bytes will be moved in their entirety,
/// and buckets larger will be split up.
const BUCKET_SPLIT_FACTOR: usize = 32;

/// The average size of values when serialized.
const AVG_VALUE_SIZE: usize = 8;

// TODO: BucketsView (plural, it's just a view into a bigger buffer of buckets)

pub fn split_by(
    buckets: &[Bucket],
    max_flush_bytes: usize,
) -> impl Iterator<Item = Vec<BucketView<'_>>> {
    CappedBucketIter::new(buckets.into_iter(), max_flush_bytes)
}

#[derive(Debug)]
pub struct BucketView<'a> {
    inner: &'a Bucket,
    range: Range<usize>,
}

impl<'a> BucketView<'a> {
    pub fn full(bucket: &'a Bucket) -> Self {
        Self {
            inner: bucket,
            range: 0..bucket.value.len(),
        }
    }

    pub fn range(bucket: &'a Bucket, range: Range<usize>) -> Self {
        // TODO this should not be exposed directly, something like bucket.split_at(X) should yield
        // two views
        Self {
            inner: bucket,
            range,
        }
    }

    pub fn len(&self) -> usize {
        self.range.len()
    }

    pub fn split_at(self, at: usize) -> (Option<Self>, Option<Self>) {
        if matches!(
            self.inner.value,
            BucketValue::Counter(_) | BucketValue::Gauge(_)
        ) {
            return (None, Some(self));
        }

        // TODO: optimize case where at is 0 and deal with at > len
        // TODO: do we need some range validations here?
        let left = Self {
            inner: self.inner,
            range: self.range.start..self.range.start + at,
        };
        let right = Self {
            inner: self.inner,
            range: self.range.start + at..self.range.end,
        };

        (Some(left), Some(right))
    }
}

impl From<&BucketView<'_>> for Bucket {
    fn from(value: &BucketView<'_>) -> Self {
        // short circuit, it's the entire bucket
        if value.range == (0..value.inner.value.len()) {
            return value.inner.clone();
        }

        let bucket_value = match &value.inner.value {
            BucketValue::Counter(c) => BucketValue::Counter(*c),
            BucketValue::Distribution(d) => BucketValue::Distribution(
                d.iter()
                    .skip(value.range.start)
                    .take(value.range.len())
                    .copied()
                    .collect(),
            ),
            BucketValue::Set(s) => BucketValue::Set(
                s.iter()
                    .skip(value.range.start)
                    .take(value.range.len())
                    .cloned()
                    .collect(),
            ),
            BucketValue::Gauge(g) => BucketValue::Gauge(*g),
        };

        Bucket {
            timestamp: value.inner.timestamp,
            width: value.inner.width,
            name: value.inner.name.clone(),
            value: bucket_value,
            tags: value.inner.tags.clone(),
        }
    }
}

impl<'a> Serialize for BucketView<'a> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let Bucket {
            timestamp,
            width,
            name,
            value,
            tags,
        } = self.inner;

        let mut state = serializer.serialize_map(Some(5))?;

        state.serialize_entry("timestamp", timestamp)?;
        state.serialize_entry("width", width)?;
        state.serialize_entry("name", name)?;

        // TODO actully slice the value
        value.serialize(serde::__private::ser::FlatMapSerializer(&mut state))?;

        if !tags.is_empty() {
            state.serialize_entry("tags", tags)?;
        }

        state.end()
    }
}

/// An iterator returning batches of buckets fitting into a size budget.
///
/// The size budget is given through `max_flush_bytes`, though this is an approximate number. On
/// every iteration, this iterator returns a `Vec<Bucket>` which serializes into a buffer of the
/// specified size. Buckets at the end of each batch may be split to fit into the batch.
///
/// Since this uses an approximate function to estimate the size of buckets, the actual serialized
/// payload may exceed the size. The estimation function is built in a way to guarantee the same
/// order of magnitude.
struct CappedBucketIter<'a, I> {
    buckets: I,
    next_bucket: Option<BucketView<'a>>,
    max_flush_bytes: usize,
}

impl<'a, I> CappedBucketIter<'a, I>
where
    I: Iterator<Item = &'a Bucket>,
{
    pub fn new(mut buckets: I, max_flush_bytes: usize) -> Self {
        let next_bucket = buckets.next().map(BucketView::full);

        Self {
            buckets,
            next_bucket,
            max_flush_bytes,
        }
    }
}

impl<'a, I> Iterator for CappedBucketIter<'a, I>
where
    I: Iterator<Item = &'a Bucket>,
{
    type Item = Vec<BucketView<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut current_batch = Vec::new();
        let mut remaining_bytes = self.max_flush_bytes;

        relay_log::info!("prepare batch");
        while let Some(bucket) = self.next_bucket.take() {
            let bucket_size = estimate_size(&bucket);
            if bucket_size <= remaining_bytes {
                relay_log::info!("add to batch");
                // the bucket fits
                remaining_bytes -= bucket_size;
                current_batch.push(bucket);
                self.next_bucket = self.buckets.next().map(BucketView::full);
            } else if bucket_size < self.max_flush_bytes / BUCKET_SPLIT_FACTOR {
                // the bucket is too small to split, move it entirely
                relay_log::info!("too small move to next");
                self.next_bucket = Some(bucket);
                break;
            } else {
                relay_log::info!("split");
                // the bucket is big enough to split
                let (left, right) = split_at(bucket, remaining_bytes);
                if let Some(left) = left {
                    current_batch.push(left);
                }

                self.next_bucket = right;
                break;
            }
        }

        if current_batch.is_empty() {
            // There is still leftover data not returned by the iterator after it has ended.
            if self.next_bucket.take().is_some() {
                relay_log::error!("CappedBucketIter swallowed bucket");
            }
            None
        } else {
            Some(current_batch)
        }
    }
}

// impl<T: Iterator<Item = Bucket>> FusedIterator for CappedBucketIter<T> {}

/// Splits this bucket if its estimated serialization size exceeds a threshold.
///
/// There are three possible return values:
///  - `(Some, None)` if the bucket fits entirely into the size budget. There is no split.
///  - `(None, Some)` if the size budget cannot even hold the bucket name and tags. There is no
///    split, the entire bucket is moved.
///  - `(Some, Some)` if the bucket fits partially. Remaining values are moved into a new bucket
///    with all other information cloned.
///
/// This is an approximate function. The bucket is not actually serialized, but rather its
/// footprint is estimated through the number of data points contained. See
/// `estimate_size` for more information.
fn split_at<'a>(
    bucket: BucketView<'a>,
    size: usize,
) -> (Option<BucketView<'a>>, Option<BucketView<'a>>) {
    // If there's enough space for the entire bucket, do not perform a split.
    if size >= estimate_size(&bucket) {
        return (Some(bucket), None);
    }

    // If the bucket key can't even fit into the remaining length, move the entire bucket into
    // the right-hand side.
    let own_size = estimate_base_size(&bucket);
    if size < (own_size + AVG_VALUE_SIZE) {
        dbg!(
            "this is fishy",
            size,
            own_size,
            AVG_VALUE_SIZE,
            own_size + AVG_VALUE_SIZE
        );
        // split_at must not be zero
        return (None, Some(bucket));
    }

    // Perform a split with the remaining space after adding the key. We assume an average
    // length of 8 bytes per value and compute the number of items fitting into the left side.
    let split_at = (size - own_size) / AVG_VALUE_SIZE;

    bucket.split_at(split_at)
}

/// Estimates the number of bytes needed to serialize the bucket without value.
///
/// Note that this does not match the exact size of the serialized payload. Instead, the size is
/// approximated through tags and a static overhead.
fn estimate_base_size(bucket: &BucketView<'_>) -> usize {
    // TODO inner
    50 + bucket.inner.name.len() + aggregator::tags_cost(&bucket.inner.tags)
}

/// Estimates the number of bytes needed to serialize the bucket.
///
/// Note that this does not match the exact size of the serialized payload. Instead, the size is
/// approximated through the number of contained values, assuming an average size of serialized
/// values.
fn estimate_size(bucket: &BucketView<'_>) -> usize {
    estimate_base_size(bucket) + bucket.len() * AVG_VALUE_SIZE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capped_iter_empty() {
        let buckets = vec![];

        let mut iter = CappedBucketIter::new(buckets.into_iter(), 200);
        assert!(iter.next().is_none());
    }

    #[test]
    fn test_capped_iter_single() {
        let json = r#"[
          {
            "name": "endpoint.response_time",
            "unit": "millisecond",
            "value": [36, 49, 57, 68],
            "type": "d",
            "timestamp": 1615889440,
            "width": 10,
            "tags": {
                "route": "user_index"
            }
          }
        ]"#;

        let buckets = serde_json::from_str::<Vec<Bucket>>(json).unwrap();

        let mut iter = CappedBucketIter::new(buckets.iter(), 200);
        let batch = iter.next().unwrap();
        assert_eq!(batch.len(), 1);

        assert!(iter.next().is_none());
    }

    #[test]
    fn test_capped_iter_split() {
        let json = r#"[
          {
            "name": "endpoint.response_time",
            "unit": "millisecond",
            "value": [1, 1, 1, 1],
            "type": "d",
            "timestamp": 1615889440,
            "width": 10,
            "tags": {
                "route": "user_index"
            }
          }
        ]"#;

        let buckets = serde_json::from_str::<Vec<Bucket>>(json).unwrap();

        // 58 is a magic number obtained by experimentation that happens to split this bucket
        let mut iter = CappedBucketIter::new(buckets.iter(), 108);
        let batch1 = iter.next().unwrap();
        assert_eq!(batch1.len(), 1);

        match Bucket::from(batch1.first().unwrap()).value {
            BucketValue::Distribution(ref dist) => assert_eq!(dist.len(), 2),
            _ => unreachable!(),
        }

        let batch2 = iter.next().unwrap();
        assert_eq!(batch2.len(), 1);

        match Bucket::from(batch2.first().unwrap()).value {
            BucketValue::Distribution(ref dist) => assert_eq!(dist.len(), 2),
            _ => unreachable!(),
        }

        assert!(iter.next().is_none());
    }

    #[test]
    fn test_capped_i_am_mentally_capped() {
        let json = r#"[
          {
            "timestamp": 1698677720,
            "width":10,
            "name":"d:custom/endpoint.response_time@millisecond",
            "type":"d",
            "value":[
              1.0,2.0,3.0,4.0,5.0,6.0,7.0,8.0,9.0,
              10.0,11.0,12.0,13.0,14.0,15.0,16.0,17.0,18.0,19.0,
              20.0,21.0,22.0,23.0,24.0,25.0,26.0,27.0,28.0,29.0,
              30.0,31.0,32.0,33.0,34.0,35.0,36.0,49.0,57.0,68.0
            ],
            "tags":{"route":"user_index"}
          }
        ]"#;

        let buckets = serde_json::from_str::<Vec<Bucket>>(json).unwrap();
        let mut iter = split_by(&buckets, 100);
        dbg!(iter.next().unwrap());

        panic!("I bims");
    }

    fn test_capped_iter_completeness(max_flush_bytes: usize, expected_elements: usize) {
        let json = r#"[
          {
            "name": "endpoint.response_time",
            "unit": "millisecond",
            "value": [1, 1, 1, 1],
            "type": "d",
            "timestamp": 1615889440,
            "width": 10,
            "tags": {
                "route": "user_index"
            }
          }
        ]"#;

        let buckets = serde_json::from_str::<Vec<Bucket>>(json).unwrap();

        let mut iter = CappedBucketIter::new(buckets.iter(), max_flush_bytes);
        let batches = iter
            .by_ref()
            .take(expected_elements + 1)
            .collect::<Vec<_>>();
        assert!(
            batches.len() <= expected_elements,
            "Cannot have more buckets than individual values"
        );
        let total_elements: usize = batches.iter().flatten().map(|x| x.len()).sum();
        assert_eq!(total_elements, expected_elements);

        let total_elements_buckets: usize = batches
            .iter()
            .flatten()
            .map(Bucket::from)
            .map(|x| x.value.len())
            .sum();

        assert_eq!(total_elements_buckets, expected_elements);
    }

    #[test]
    fn test_capped_iter_completeness_0() {
        test_capped_iter_completeness(0, 0);
    }

    #[test]
    fn test_capped_iter_completeness_90() {
        // This would cause an infinite loop.
        test_capped_iter_completeness(90, 0);
    }

    #[test]
    fn test_capped_iter_completeness_100() {
        test_capped_iter_completeness(100, 4);
    }
}
