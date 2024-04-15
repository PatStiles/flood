use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::str::FromStr;

use anyhow::anyhow;
use chrono::Utc;
use clap::Parser;
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use serde_json::value::Value;

/// Controls how long the benchmark should run.
/// We can specify either a time-based duration or a number of calls to perform.
/// It is also used for controlling sampling.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum Interval {
    Count(u64),
    Time(tokio::time::Duration),
    Unbounded,
}

impl Interval {
    pub fn is_not_zero(&self) -> bool {
        match self {
            Interval::Count(cnt) => *cnt > 0,
            Interval::Time(d) => !d.is_zero(),
            Interval::Unbounded => false,
        }
    }

    pub fn is_bounded(&self) -> bool {
        !matches!(self, Interval::Unbounded)
    }

    pub fn count(&self) -> Option<u64> {
        if let Interval::Count(c) = self {
            Some(*c)
        } else {
            None
        }
    }

    pub fn seconds(&self) -> Option<f32> {
        if let Interval::Time(d) = self {
            Some(d.as_secs_f32())
        } else {
            None
        }
    }
}

/// If the string is a valid integer, it is assumed to be the number of cycles.
/// If the string additionally contains a time unit, e.g. "s" or "secs", it is parsed
/// as time duration.
impl FromStr for Interval {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Ok(i) = s.parse() {
            Ok(Interval::Count(i))
        } else if let Ok(d) = parse_duration::parse(s) {
            Ok(Interval::Time(d))
        } else {
            Err("Required integer number of cycles or time duration".to_string())
        }
    }
}

// Taken from cast cli: https://github.com/foundry-rs/foundry/blob/master/crates/cast/bin/cmd/rpc.rs
/// CLI arguments for `cast rpc`.
#[derive(Parser, Clone, Debug, Serialize, Deserialize)]
pub struct RpcCommand {
    /// RPC method name
    method: String,

    /// RPC parameters
    ///
    /// Interpreted as JSON:
    ///
    /// flood rpc eth_getBlockByNumber 0x123 false
    /// => {"method": "eth_getBlockByNumber", "params": ["0x123", false] ... }
    /// 
    /// flood rpc eth_getBlockByNumber 0x123 false
    pub params: Vec<String>,

    /// Send raw JSON parameters
    ///
    /// The first param will be interpreted as a raw JSON array of params.
    /// If no params are given, stdin will be used. For example:
    ///
    /// flood rpc eth_getBlockByNumber '["0x123", false]' --raw
    ///     => {"method": "eth_getBlockByNumber", "params": ["0x123", false] ... }
    #[clap(long, short = 'j')]
    raw: bool,

    // RUN COMMANDS
    /// Number of cycles per second to execute.
    /// If not given, the benchmark cycles will be executed as fast as possible.
    // TODO: add reserved word for logarithmic ramp up
    #[clap(short('r'), long, value_name = "COUNT", num_args(0..))]
    pub rate: Option<Vec<f64>>,

    /// Number of cycles or duration of the warmup phase.
    #[clap(
        short('w'),
        long("warmup"),
        default_value = "1",
        value_name = "TIME | COUNT"
    )]
    pub warmup_duration: Interval,

    /// Number of cycles or duration of the main benchmark phase.
    #[clap(
        short('d'),
        long("duration"),
        default_value = "60s",
        value_name = "TIME | COUNT"
    )]
    pub run_duration: Interval,

    /// Number of worker threads used by the driver.
    #[clap(short('t'), long, default_value = "1", value_name = "COUNT")]
    pub threads: NonZeroUsize,

    /// Max number of concurrent async requests per thread during the main benchmark phase.
    #[clap(short('p'), long, default_value = "128", value_name = "COUNT")]
    pub concurrency: NonZeroUsize,

    /// Throughput sampling period, in seconds.
    #[clap(
        short('s'),
        long("sampling"),
        default_value = "1s",
        value_name = "TIME | COUNT"
    )]
    pub sampling_interval: Interval,

    /// Label that will be added to the report to help identifying the test
    #[clap(long("tag"), number_of_values = 1)]
    pub tags: Vec<String>,

    /// Path to an output file or directory where the JSON report should be written to.
    #[clap(short('o'), long)]
    #[serde(skip)]
    pub output: Option<PathBuf>,

    /// Path to a report from another earlier run that should be compared to side-by-side
    #[clap(short('b'), long, value_name = "PATH")]
    pub baseline: Option<PathBuf>,

    /// Don't display the progress bar.
    #[clap(short, long)]
    pub quiet: bool,

    // Cassandra connection settings.
    #[clap(short('u'), long, num_args(0..))]
    pub rpc_url: Option<Vec<String>>,

    /// Seconds since 1970-01-01T00:00:00Z
    #[clap(hide = true, long)]
    pub timestamp: Option<i64>,

    #[clap(skip)]
    pub cluster_name: Option<String>,

    #[clap(skip)]
    pub chain_id: Option<String>,
}

/*
TODO: 
- north star = be able to collect a single production quality dataset
- main remaining goal = be able to make multiple calls using different parameter values for the same method
    - Parse range of parameters for call to create workload
    - Have Flag to execute using batching -> sub parameter than specifies number of calls in batch, default is max
        - Warning that specifies
    - Parse file of parameters to create workload
    - Parse multiple calls and params to create workload
    - Parse multiple calls and params from a file to create a workload
    TO FUCK THIS PIG:
        - Parsing:
            - Three formats with sub flag
                - List
                    - Delimiter: {[], }
                - Range
                    - Delimiter: 0..1, 0x12..
                - Random/Any
                    - Delimiter: *
                - Random/Exclusive
                    - Delimiter: ????
        - Workload:
            - Have Vec of Different created requests.
        - Execution:
            - iterate and execute within workload
- quality of life things = add example usage to readme + allow easier quitting with control c
    - Parse file of parameters to create workload
- Build batched JSON-RPC tests
    - Parse file of parameters to create workload
*/
impl RpcCommand {
    // Parses an individual JSON-RPC call as defined in the clap interface
    pub fn parse_rpc_call(&self) -> Result<(String, Value), anyhow::Error> { 
        let RpcCommand {
            raw,
            method,
            params,
            ..
        } = self;

        let params = if *raw {
            if params.is_empty() {
                serde_json::Deserializer::from_reader(std::io::stdin())
                    .into_iter()
                    .next()
                    .transpose()?
                    .ok_or_else(|| anyhow!("Empty JSON parameters"))?
            } else {
                value_or_string(params.iter().join(" "))
            }
        } else {
            //TODO: remove this clone
            serde_json::Value::Array(
                params
                    .iter()
                    .map(|value: &String| value_or_string(value.clone()))
                    .collect(),
            )
        };
        Ok((method.to_string(), params))
    }

    pub fn parse_params(&self) -> Result<Vec<(String, Value)>, anyhow::Error> {

        let requests = self.parse_rpc_call().unwrap();
        Ok(vec![requests])
    }

    pub fn set_timestamp_if_empty(mut self) -> Self {
        if self.timestamp.is_none() {
            self.timestamp = Some(Utc::now().timestamp())
        }
        self
    }

    /// Returns benchmark name
    pub fn name(&self) -> String {
        self.method.clone()
    }

    /// Suggested file name where to save the results of the run.
    pub fn default_output_file_name(&self, extension: &str) -> PathBuf {
        let mut components = vec![self.name()];
        components.extend(self.cluster_name.iter().map(|x| x.replace(' ', "_")));
        components.extend(self.chain_id.iter().cloned());
        components.extend(self.tags.iter().cloned());
        //components.extend(self.rate.map(|r| format!("r{r}")));
        components.push(format!("p{}", self.concurrency));
        components.push(format!("t{}", self.threads));
        components.push(chrono::Local::now().format("%Y%m%d.%H%M%S").to_string());
        PathBuf::from(format!("{}.{extension}", components.join(".")))
    }
}

fn value_or_string(value: String) -> Value {
    serde_json::from_str(&value).unwrap_or(serde_json::Value::String(value))
}

#[derive(Parser, Debug)]
pub struct ShowCommand {
    /// Path to the JSON report file
    #[clap(value_name = "PATH")]
    pub report: PathBuf,

    /// Optional path to another JSON report file
    #[clap(short('b'), long, value_name = "PATH")]
    pub baseline: Option<PathBuf>,
}

#[derive(Parser, Debug)]
pub struct HdrCommand {
    /// Path to the input JSON report file
    #[clap(value_name = "PATH")]
    pub report: PathBuf,

    /// Output file; if not given, the hdr log gets printed to stdout
    #[clap(short('o'), long, value_name = "PATH")]
    pub output: Option<PathBuf>,

    /// Optional tag prefix to add to each histogram
    #[clap(long, value_name = "STRING")]
    pub tag: Option<String>,
}

#[derive(Parser, Debug)]
pub struct PlotCommand {
    /// Path to the input JSON report file(s)
    #[clap(value_name = "PATH", required = true)]
    pub reports: Vec<PathBuf>,

    /// Plot given response time percentiles. Can be used multiple times.
    #[clap(short, long("percentile"), number_of_values = 1)]
    pub percentiles: Vec<f64>,

    /// Plot throughput.
    #[clap(short, long("throughput"))]
    pub throughput: bool,

    /// Write output to the given file.
    #[clap(short('o'), long, value_name = "PATH")]
    pub output: Option<PathBuf>,
}

#[derive(Parser, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum Command {
    /// Displays the report(s) of previously executed benchmark(s).
    ///
    /// Can compare two runs.
    Show(ShowCommand),

    /// Exports histograms as a compressed HDR interval log.
    ///
    /// To be used with HdrHistogram (https://github.com/HdrHistogram/HdrHistogram).
    /// Timestamps are given in seconds since Unix epoch.
    /// Response times are recorded in nanoseconds.
    Hdr(HdrCommand),

    /// Plots recorded samples. Saves output in SVG format.
    Plot(PlotCommand),

    /// Runs a benchmark on a single specified JSON-RPC
    ///
    /// Prints nicely formatted statistics to the standard output.
    /// Additionally dumps all data into a JSON report file.
    Rpc(RpcCommand),
}

#[derive(Parser, Debug)]
#[command(
name = "Ethereum Node Latency and Throughput Tester",
author = "Patrick Stiles <https://github.com/PatStiles>",
version = clap::crate_version ! (),
)]
pub struct AppConfig {
    #[clap(subcommand)]
    pub command: Command,
}

#[derive(Debug, Deserialize, Default)]
pub struct SchemaConfig {
    #[serde(default)]
    pub script: Vec<String>,
    #[serde(default)]
    pub cql: String,
}

#[derive(Debug, Deserialize)]
pub struct LoadConfig {
    pub count: u64,
    #[serde(default)]
    pub script: Vec<String>,
    #[serde(default)]
    pub cql: String,
}

mod defaults {
    pub fn ratio() -> f64 {
        1.0
    }
}

#[derive(Debug, Deserialize)]
pub struct RunConfig {
    #[serde(default = "defaults::ratio")]
    pub ratio: f64,
    #[serde(default)]
    pub script: Vec<String>,
    #[serde(default)]
    pub cql: String,
}

#[derive(Debug, Deserialize)]
pub struct WorkloadConfig {
    #[serde(default)]
    pub schema: SchemaConfig,
    #[serde(default)]
    pub load: HashMap<String, LoadConfig>,
    pub run: HashMap<String, RunConfig>,
    #[serde(default)]
    pub bindings: HashMap<String, String>,
}
