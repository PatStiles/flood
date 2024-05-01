use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
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

fn parse_range(input: &str) -> Result<Vec<String>, &'static str> {
    let parts: Vec<&str> = input.split("..").collect();

    if parts.len() != 2 {
        return Err("Invalid range format. Use START..END or START..=END");
    }

    let inclusive = parts[1].starts_with('=');
    let start = i32::from_str_radix(parts[0].trim_start_matches("0x"), 16).unwrap();
    let end = if inclusive {
        i32::from_str_radix(&parts[1].trim_start_matches("=0x"), 16).unwrap()
    } else {
        i32::from_str_radix(parts[1].trim_start_matches("0x"), 16).unwrap()
    };

    if start > end {
        return Err("Start value cannot be greater than end value");
    }

    let range: Vec<String> = if inclusive {
        (start..=end).map(|x| format!("0x{:02x}", x)).collect()
    } else {
        (start..end).map(|x| format!("0x{:02x}", x)).collect()
    };

    Ok(range)
}

fn parse_params(s: &str) -> Result<Vec<String>, String> {
    Ok(s.split(' ').map(|s| s.to_string()).collect())
}

// Taken from cast cli: https://github.com/foundry-rs/foundry/blob/master/crates/cast/bin/cmd/rpc.rs
/// CLI arguments for `cast rpc`.
#[derive(Parser, Clone, Debug, Serialize, Deserialize)]
pub struct RpcCommand {
    /// RPC method name
    #[arg(required_unless_present = "input")]
    method: Option<String>,

    /// RPC parameters
    ///
    /// Interpreted as JSON:
    ///
    /// flood rpc eth_getBlockByNumber 0x123 false
    /// => {"method": "eth_getBlockByNumber", "params": ["0x123", false] ... }
    ///
    /// flood rpc eth_getBlockByNumber 0x123 false
    //TODO: this accepts any number of params and parses shouldn't???
    #[arg(
        required_unless_present = "input",
        value_parser(parse_params),
        value_delimiter = ','
    )]
    pub params: Option<Vec<Vec<String>>>,

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

    /// Path to JSON input file with JSON-RPC calls
    #[clap(short('i'), long)]
    #[serde(skip)]
    pub input: Option<PathBuf>,

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

    //TODO: add default value
    /// Eth Node RPC-URL
    #[clap(short('u'), long, num_args(0..))]
    pub rpc_url: Option<Vec<String>>,

    #[clap(short('e'), long)]
    pub exp_ramp: Option<u64>,

    /// Seconds since 1970-01-01T00:00:00Z
    #[clap(hide = true, long)]
    pub timestamp: Option<i64>,

    /// Number of requests per workload set during parse_params
    #[clap(hide = true, long, default_value = "1")]
    pub num_req: Option<usize>,

    #[clap(skip)]
    pub cluster_name: Option<String>,

    #[clap(skip)]
    pub chain_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct JsonRequest {
    method: String,
    params: serde_json::Value,
}

/*
TODO:
- north star = be able to collect a single production quality dataset
- main remaining goal = be able to make multiple calls using different parameter values for the same method
    - Parse range of parameters for call to create workload
    - Parse multiple calls and params from a file to create a workload
*/
impl RpcCommand {
    fn parse_rpc_params(params: &Vec<String>, raw: &bool) -> Result<Value, anyhow::Error> {
        let params = if *raw {
            if params.is_empty() {
                serde_json::Deserializer::from_reader(std::io::stdin())
                    .into_iter()
                    .next()
                    .transpose()?
                    .ok_or_else(|| anyhow!("Empty JSON parameters"))?
            } else {
                Self::value_or_string(params.iter().join(" "))
            }
        } else {
            //TODO: remove this clone
            serde_json::Value::Array(
                params
                    .iter()
                    .map(|value: &String| Self::value_or_string(value.clone()))
                    .collect(),
            )
        };
        Ok(params)
    }

    fn value_or_string(value: String) -> Value {
        serde_json::from_str(&value).unwrap_or(serde_json::Value::String(value))
    }

    fn parse_file(path: &PathBuf) -> Vec<(String, Value)> {
        // Check if the specified file exists and is a .json file
        if let Some(extension) = path.extension() {
            if extension != "json" {
                eprintln!("Error: File is not a .json file");
                std::process::exit(1);
            }
        } else {
            eprintln!("Error: File does not have an extension");
            std::process::exit(1);
        }

        // Read the contents of the file
        let mut file = match File::open(&path) {
            Ok(file) => file,
            Err(_) => {
                eprintln!("Error: Failed to open the file");
                std::process::exit(1);
            }
        };

        let mut contents = String::new();

        if let Err(_) = file.read_to_string(&mut contents) {
            eprintln!("Error: Failed to read the file");
            std::process::exit(1);
        }

        // Parse JSON-RPC requests
        let json_requests: Vec<JsonRequest> = match serde_json::from_str(&contents) {
            Ok(json_requests) => json_requests,
            Err(_) => {
                eprintln!("Error: Failed to parse JSON");
                std::process::exit(1);
            }
        };

        // Extract method and params from each request
        // TODO: verify this works for Value and String types...
        let parsed_requests: Vec<(String, serde_json::Value)> = json_requests
            .iter()
            .map(|req| (req.method.clone(), req.params.clone()))
            .collect();

        parsed_requests
    }

    pub fn parse_params(&self) -> Result<Vec<(String, Value)>, anyhow::Error> {
        let RpcCommand {
            raw,
            method,
            params,
            input,
            ..
        } = self;

        if let Some(path) = input {
            let reqs = Self::parse_file(path);
            Ok(reqs)
        } else {
            //TODO: remove unwrap -> Claps interface requires this for optional args
            //TODO: abstract this into a generate args function -> maybne leverage arbitrary
            let params = params.as_ref().unwrap();
            let method = method.as_ref().unwrap();
            let params = params.iter().fold(Vec::new(), |mut acc, param| {
                let mut has_range = false;
                for (j, token) in param.iter().enumerate() {
                    //TODO: generalize so it so it records range values and param indexes then generates data from that new set.
                    if token.contains("..") {
                        has_range = true;
                        let range = parse_range(token).unwrap();
                        for val in range {
                            let mut new_param = param.clone();
                            new_param[j] = val.clone();
                            acc.push(new_param);
                        }
                        //For now we only allow one range per parameters
                        break;
                    }
                }
                if has_range {
                    acc
                } else {
                    acc.push(param.clone());
                    acc
                }
            });
            let reqs: Vec<(String, Value)> = params
                .iter()
                .map(|param| (method.clone(), Self::parse_rpc_params(&param, raw).unwrap()))
                .collect();
            Ok(reqs)
        }
    }

    pub fn set_timestamp_if_empty(mut self) -> Self {
        if self.timestamp.is_none() {
            self.timestamp = Some(Utc::now().timestamp())
        }
        self
    }

    pub fn set_num_req(mut self, num_req: usize) -> Self {
        if self.num_req.is_none() {
            self.num_req = Some(num_req)
        }
        self
    }

    /// Parses rate for run
    pub fn parse_rate(&self) -> Option<Vec<f64>> {
        let num_req = self.num_req.unwrap();
        if let Some(max_rate) = self.exp_ramp {
            let num_values = 10;
            let mut log_rates = Vec::with_capacity(num_values);
            let start_rate = (10 / num_req) as f64;
            let max_rate = (max_rate / num_req as u64) as f64;
            for i in 0..num_values {
                let ratio = i as f64 / (num_values - 1) as f64;
                let rate = start_rate
                    * std::f64::consts::E.powf((max_rate.log10() - start_rate.log10()) * ratio);
                log_rates.push(rate);
            }

            return Some(log_rates)
        }
        // If not set return None
        if let Some(rate) = &self.rate {
            Some(rate.into_iter().map(|r| r / num_req as f64).collect())
        } else {
            None
        }
    }

    /// Returns benchmark name
    pub fn name(&self) -> String {
        //TODO: address this mess
        self.method
            .as_ref()
            .unwrap_or(&self.input.as_ref().unwrap().to_str().unwrap().to_string())
            .clone()
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
