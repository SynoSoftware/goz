//! What the benches measure. On Windows it is the CPU cycles of the bench
//! thread, because the machine they run on is rarely idle and other work
//! moves wall-clock timings. Elsewhere it is wall time.

use criterion::Criterion;

#[cfg(windows)]
pub(crate) type Measure = cycles::Cycles;
#[cfg(not(windows))]
pub(crate) type Measure = criterion::measurement::WallTime;

pub(crate) fn criterion() -> Criterion<Measure> {
    #[cfg(windows)]
    let measure = cycles::Cycles;
    #[cfg(not(windows))]
    let measure = criterion::measurement::WallTime;
    Criterion::default().with_measurement(measure)
}

#[cfg(windows)]
mod cycles {
    use criterion::Throughput;
    use criterion::measurement::{Measurement, ValueFormatter};

    /// CPU cycles of the bench thread ([`gozler_winfs::thread_cycles`]).
    pub(crate) struct Cycles;

    impl Measurement for Cycles {
        type Intermediate = u64;
        type Value = u64;
        fn start(&self) -> u64 {
            gozler_winfs::thread_cycles()
        }
        fn end(&self, start: u64) -> u64 {
            gozler_winfs::thread_cycles() - start
        }
        fn add(&self, a: &u64, b: &u64) -> u64 {
            a + b
        }
        fn zero(&self) -> u64 {
            0
        }
        fn to_f64(&self, cycles: &u64) -> f64 {
            *cycles as f64
        }
        fn formatter(&self) -> &dyn ValueFormatter {
            &Megacycles
        }
    }

    struct Megacycles;

    impl ValueFormatter for Megacycles {
        fn scale_values(&self, _typical: f64, values: &mut [f64]) -> &'static str {
            values.iter_mut().for_each(|v| *v /= 1e6);
            "Mcycles"
        }
        fn scale_throughputs(&self, _: f64, _: &Throughput, values: &mut [f64]) -> &'static str {
            values.iter_mut().for_each(|v| *v /= 1e6);
            "Mcycles"
        }
        fn scale_for_machines(&self, _: &mut [f64]) -> &'static str {
            "cycles"
        }
    }
}
