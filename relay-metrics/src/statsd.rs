use relay_statsd::{CounterMetric, GaugeMetric, HistogramMetric, TimerMetric};

/// Counter metrics for Relay Metrics.
pub enum MetricCounters {
    /// Incremented for every metric that is inserted.
    ///
    /// Tagged by metric type and name.
    InsertMetric,

    /// Incremented every time two buckets or two metrics are merged.
    ///
    /// Tagged by metric type and name.
    MergeHit,

    /// Incremented every time a bucket is created.
    ///
    /// Tagged by metric type and name.
    MergeMiss,
}

impl CounterMetric for MetricCounters {
    fn name(&self) -> &'static str {
        match *self {
            Self::InsertMetric => "metrics.insert",
            Self::MergeHit => "metrics.buckets.merge.hit",
            Self::MergeMiss => "metrics.buckets.merge.miss",
        }
    }
}

/// Timer metrics for Relay Metrics.
pub enum MetricTimers {
    /// Time in milliseconds spent scanning metric buckets to flush.
    ///
    /// Relay scans metric buckets in regular intervals and flushes expired buckets. This timer
    /// shows the time it takes to perform this scan and remove the buckets from the internal cache.
    /// Sending the metric buckets to upstream is outside of this timer.
    BucketsScanDuration,
}

impl TimerMetric for MetricTimers {
    fn name(&self) -> &'static str {
        match *self {
            Self::BucketsScanDuration => "metrics.buckets.scan_duration",
        }
    }
}

/// Histogram metrics for Relay Metrics.
pub enum MetricHistograms {
    /// The total number of metric buckets flushed in a cycle across all projects.
    BucketsFlushed,

    /// The number of metric buckets flushed in a cycle for each project.
    ///
    /// Relay scans metric buckets in regular intervals and flushes expired buckets. This histogram
    /// is logged for each project that is being flushed. The count of the histogram values is
    /// equivalent to the number of projects being flushed.
    BucketsFlushedPerProject,
}

impl HistogramMetric for MetricHistograms {
    fn name(&self) -> &'static str {
        match *self {
            Self::BucketsFlushed => "metrics.buckets.flushed",
            Self::BucketsFlushedPerProject => "metrics.buckets.flushed_per_project",
        }
    }
}

/// Gauge metrics for Relay Metrics.
pub enum MetricGauges {
    /// The total number of metric buckets in Relay's metrics aggregator.
    Buckets,
}

impl GaugeMetric for MetricGauges {
    fn name(&self) -> &'static str {
        match *self {
            Self::Buckets => "metrics.buckets",
        }
    }
}