# Flood Rust:
`Flood Rust` is a Rust rewrite of Flood based on [Latte](https://github.com/pkolaczk/latte) a performance benchmarking tool for CassandraDB. Currently, `flood_rust` can benchmark RPC node performance of individual JSON-RPC requests and series of JSON-RPC requests across a variety of parameters inputed manually or from an input file. For each benchmark, `flood_rust` defines a `Workload` which may contain one or more JSON-RPC requests and repeatedly executes cycles of the workload. The user may define the `--rate  [call/s]`. The execution time of a `Workload` cycle are recorded as well as the timing and success of individual JSON-RPC calls are recorded.

## Examples:

[JSON-RPC Reference](https://ethereum.org/en/developers/docs/apis/json-rpc)

#### Single JSON-RPC
```bash
cargo b --bins

target/debug/flood rpc eth_getBlockByNumber "0x1b4 true" --rpc-url [<RPC_URL>..] --rate 100
target/debug/flood rpc eth_getStorageAt "0x295a70b2de5e3953354a6a8344e616ed314d7251 0x0 latest" --rpc-url [<RPC_URL>..] --rate 100
```

#### Single JSON-RPC with expoential ramp up -> Generates a list of rates up to the max that fit log10 curve.
```bash
cargo b --bins

target/debug/flood rpc eth_getBlockByNumber "0x1b4 true" --rpc-url [<RPC_URL>..] --exp_ramp 5000
target/debug/flood rpc eth_getStorageAt "0x295a70b2de5e3953354a6a8344e616ed314d7251 0x0 latest" --rpc-url [<RPC_URL>..] --exp_ramp 5000
```

#### Multiple JSON-RPC requests with different parameters executed serially
```bash
cargo b --bins

target/debug/flood rpc eth_getBlockByNumber "0x1b4 true","0x242 true" --rpc-url [<RPC_URL>..] --rate 100 --random
```

#### Multiple JSON-RPC requests executed in random order on each workload cycle
```bash
cargo b --bins

target/debug/flood rpc eth_getBlockByNumber "0x1b4 true","0x242 true" --rpc-url [<RPC_URL>..] --rate 100
```

#### Select and execute a single random requests per cycle from a list of multiple requests
```bash
cargo b --bins

target/debug/flood rpc eth_getBlockByNumber "0x1b4 true","0x242 true" --rpc-url [<RPC_URL>..] --rate 100 --choose
```


#### Multiple JSON-RPC requests over a range of parameters (Supports ranges for a single parameter within a list, multiple ranged parameters not allowed)
```bash
cargo b --bins

target/debug/flood rpc eth_getBlockByNumber "0x1b4..0x1bb true","0x242..0x24b true" --rpc-url [<RPC_URL>..] --rate 100
```

#### Multiple JSON-RPC requests from file
```bash
cargo b --bins
target/debug/flood rpc --input examples/eth_getBlockByNumber.json --rpc-url [<RPC_URL>..] --rate 100
target/debug/flood rpc --input examples/eth_getStorageAt.json --rpc-url [<RPC_URL>..] --rate 100
```