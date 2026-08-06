//! Service counters plus a few host gauges, rendered in Prometheus text
//! format by the API. The appliance exposes exactly one port, so host
//! metrics that would otherwise need node-exporter (load, memory, spool
//! usage) are read from /proc at render time and served here too.

use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
pub struct Metrics {
    pub ssdp_airwave_msearch: AtomicU64,
    pub ssdp_airwave_notify: AtomicU64,
    pub ssdp_renderer_replies: AtomicU64,
    pub ssdp_home_msearch: AtomicU64,
    pub ssdp_home_replies: AtomicU64,
    pub ssdp_dropped: AtomicU64,
    pub env_discovery_runs: AtomicU64,
    pub env_devices: AtomicU64,
    pub env_polls_ok: AtomicU64,
    pub env_polls_failed: AtomicU64,
    pub kasa_discovery_runs: AtomicU64,
    pub kasa_devices: AtomicU64,
    pub kasa_polls_ok: AtomicU64,
    pub kasa_polls_failed: AtomicU64,
    pub spool_lines_written: AtomicU64,
    pub ingest_lines_accepted: AtomicU64,
    pub ingest_lines_rejected: AtomicU64,
    pub api_requests: AtomicU64,
    pub api_unauthorized: AtomicU64,
    pub batches_served: AtomicU64,
    pub batches_acked: AtomicU64,
}

pub fn inc(counter: &AtomicU64) {
    counter.fetch_add(1, Ordering::Relaxed);
}

pub fn set(counter: &AtomicU64, value: u64) {
    counter.store(value, Ordering::Relaxed);
}

impl Metrics {
    pub fn render(&self, spools: &crate::spool::SpoolSet) -> String {
        let mut out = String::new();
        let mut emit = |name: &str, value: u64| {
            out.push_str(name);
            out.push(' ');
            out.push_str(&value.to_string());
            out.push('\n');
        };

        emit(
            "collector_ssdp_airwave_msearch_total",
            self.ssdp_airwave_msearch.load(Ordering::Relaxed),
        );
        emit(
            "collector_ssdp_airwave_notify_total",
            self.ssdp_airwave_notify.load(Ordering::Relaxed),
        );
        emit(
            "collector_ssdp_renderer_replies_total",
            self.ssdp_renderer_replies.load(Ordering::Relaxed),
        );
        emit(
            "collector_ssdp_home_msearch_total",
            self.ssdp_home_msearch.load(Ordering::Relaxed),
        );
        emit(
            "collector_ssdp_home_replies_total",
            self.ssdp_home_replies.load(Ordering::Relaxed),
        );
        emit(
            "collector_ssdp_dropped_total",
            self.ssdp_dropped.load(Ordering::Relaxed),
        );
        emit(
            "collector_env_discovery_runs_total",
            self.env_discovery_runs.load(Ordering::Relaxed),
        );
        emit("collector_env_devices", self.env_devices.load(Ordering::Relaxed));
        emit(
            "collector_env_polls_ok_total",
            self.env_polls_ok.load(Ordering::Relaxed),
        );
        emit(
            "collector_env_polls_failed_total",
            self.env_polls_failed.load(Ordering::Relaxed),
        );
        emit(
            "collector_kasa_discovery_runs_total",
            self.kasa_discovery_runs.load(Ordering::Relaxed),
        );
        emit("collector_kasa_devices", self.kasa_devices.load(Ordering::Relaxed));
        emit(
            "collector_kasa_polls_ok_total",
            self.kasa_polls_ok.load(Ordering::Relaxed),
        );
        emit(
            "collector_kasa_polls_failed_total",
            self.kasa_polls_failed.load(Ordering::Relaxed),
        );
        emit(
            "collector_spool_lines_written_total",
            self.spool_lines_written.load(Ordering::Relaxed),
        );
        emit(
            "collector_ingest_lines_accepted_total",
            self.ingest_lines_accepted.load(Ordering::Relaxed),
        );
        emit(
            "collector_ingest_lines_rejected_total",
            self.ingest_lines_rejected.load(Ordering::Relaxed),
        );
        emit(
            "collector_api_requests_total",
            self.api_requests.load(Ordering::Relaxed),
        );
        emit(
            "collector_api_unauthorized_total",
            self.api_unauthorized.load(Ordering::Relaxed),
        );
        emit(
            "collector_batches_served_total",
            self.batches_served.load(Ordering::Relaxed),
        );
        emit(
            "collector_batches_acked_total",
            self.batches_acked.load(Ordering::Relaxed),
        );

        let stats = spools.stats();
        emit("collector_spool_bytes", stats.total_bytes);
        emit("collector_spool_closed_segments", stats.closed_segments);
        emit("collector_spool_dropped_segments_total", stats.dropped_segments);

        // Host gauges: enough to see a sick appliance without a second
        // metrics port.
        if let Ok(loadavg) = std::fs::read_to_string("/proc/loadavg") {
            if let Some(load1) = loadavg.split_whitespace().next() {
                out.push_str("collector_host_load1 ");
                out.push_str(load1);
                out.push('\n');
            }
        }
        if let Ok(meminfo) = std::fs::read_to_string("/proc/meminfo") {
            for (label, metric) in [
                ("MemTotal:", "collector_host_mem_total_kb"),
                ("MemAvailable:", "collector_host_mem_available_kb"),
            ] {
                if let Some(line) = meminfo.lines().find(|l| l.starts_with(label)) {
                    if let Some(kb) = line.split_whitespace().nth(1) {
                        out.push_str(metric);
                        out.push(' ');
                        out.push_str(kb);
                        out.push('\n');
                    }
                }
            }
        }
        out
    }
}
