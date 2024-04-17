use std::fmt::Debug;
use std::time::Duration;
use std::time::Instant;

use alloy_json_rpc::Request;
use alloy_json_rpc::RpcError;
use alloy_rpc_client::RpcCall;
use alloy_transport::TransportErrorKind;
use hdrhistogram::Histogram;
use serde_json::value::RawValue;
use try_lock::TryLock;

use crate::error::FloodError;
use crate::{Context, SessionStats};

/// Tracks statistics of the Rune function invoked by the workload
#[derive(Clone, Debug)]
pub struct FnStats {
    pub call_count: u64,
    pub call_times_ns: Histogram<u64>,
}

impl FnStats {
    pub fn operation_completed(&mut self, duration: Duration) {
        self.call_count += 1;
        self.call_times_ns
            .record(duration.as_nanos().clamp(1, u64::MAX as u128) as u64)
            .unwrap();
    }
}

impl Default for FnStats {
    fn default() -> Self {
        FnStats {
            call_count: 0,
            call_times_ns: Histogram::new(3).unwrap(),
        }
    }
}

/// Statistics of operations (function calls) and Cassandra requests.
pub struct WorkloadStats {
    pub start_time: Instant,
    pub end_time: Instant,
    pub function_stats: FnStats,
    pub session_stats: SessionStats,
}

/// Mutable part of Workload
pub struct WorkloadState {
    start_time: Instant,
    fn_stats: FnStats,
}

impl Default for WorkloadState {
    fn default() -> Self {
        WorkloadState {
            start_time: Instant::now(),
            fn_stats: Default::default(),
        }
    }
}

pub struct Workload {
    context: Context,
    state: TryLock<WorkloadState>,
    requests: Vec<Request<Box<RawValue>>>,
}

impl Workload {
    pub fn new(context: Context, requests: Vec<Request<Box<RawValue>>>) -> Workload {
        Workload {
            context,
            state: TryLock::new(WorkloadState::default()),
            requests,
        }
    }

    pub fn clone(&self) -> Result<Self, FloodError> {
        Ok(Workload {
            context: self.context.clone()?,
            // make a deep copy to avoid congestion on Arc ref counts used heavily by Rune
            state: TryLock::new(WorkloadState::default()),
            requests: self.requests.clone(),
        })
    }

    pub async fn call(&self) -> Result<(), FloodError> {
        for call in self.requests.clone() {
            let start_time = self.context.stats.try_lock().unwrap().start_request();
            // Each workload object can be a single, multiple, or batch of requests.
            // This can fuck with measurements as we basically want to define a workload of different params, bench the entire execution and the execution of individual request....
            // Have two stats... one per workload call and one per call to run() as is done within latte
            let rs: Result<Box<RawValue>, RpcError<TransportErrorKind>> =
                RpcCall::new(call, self.context.session.transport().clone())
                    .boxed()
                    .await;
            let end_time = Instant::now();
            //TAKE SESSION STATS as we don't make a Rune function call
            //NOTE: These are per call stats
            self.context
                .stats
                .try_lock()
                .unwrap()
                .complete_request::<Box<serde_json::value::RawValue>, TransportErrorKind>(
                    end_time - start_time,
                    &rs,
                );
        }
        Ok(())
    }

    /// Executes a single cycle of a workload.
    /// This should be idempotent –
    /// the generated action should be a function of the iteration number.
    /// Returns the cycle number and the end time of the query.
    // TODO: separate workload from call.
    pub async fn run(&self, cycle: u64) -> Result<(u64, Instant), FloodError> {
        let start_time = Instant::now();
        let rs = self.call().await;
        let end_time = Instant::now();
        let mut state = self.state.try_lock().unwrap();
        //NOTE: This is per workload stats
        state.fn_stats.operation_completed(end_time - start_time);

        //TODO: store and analyze results later???
        match rs {
            Ok(_) => Ok((cycle, end_time)),
            //TODO: all but eth call have "deserialization error: invalid type: boolean `false`, expected unit at line 1 column 5"
            Err(_) => Ok((cycle, end_time)),
        }
    }

    /// Returns the reference to the contained context.
    /// Allows to e.g. access context stats.
    pub fn context(&self) -> &Context {
        &self.context
    }

    /// Sets the workload start time and resets the counters.
    /// Needed for producing `WorkloadStats` with
    /// recorded start and end times of measurement.
    pub fn reset(&self, start_time: Instant) {
        let mut state = self.state.try_lock().unwrap();
        state.fn_stats = FnStats::default();
        state.start_time = start_time;
        self.context.reset_session_stats();
    }

    /// Returns statistics of the operations invoked by this workload so far.
    /// Resets the internal statistic counters.
    pub fn take_stats(&self, end_time: Instant) -> WorkloadStats {
        let mut state = self.state.try_lock().unwrap();
        let result = WorkloadStats {
            start_time: state.start_time,
            end_time,
            function_stats: state.fn_stats.clone(),
            session_stats: self.context().take_session_stats(),
        };
        state.start_time = end_time;
        state.fn_stats = FnStats::default();
        result
    }
}
