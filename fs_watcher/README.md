# File System Watcher

A simple Rust project that monitors a specified directory for file system changes such as creation. It leverages the `notify` crate to provide cross-platform file system event monitoring. Once a file is created, it analyzes the file and sends the results to a specified server.

## Features

- Monitors a specified directory for file changes
- Logs events to the console

## Dependencies

- **clap** : For command-line argument parsing
- **inotify** : For file system event monitoring
- **ctrlc** : For handling termination signals gracefully
- **rayon** : For asynchronous event handling
- **serde and serde_json** : For configuration file parsing

## Configuration

The watcher can be configured wia a configuration file (JSON format) or command-line arguments.

### Config file

```json
{
    "folder_to_scan": "./tests/watched_dir/",
    "max_thread": 5,
    "server_ip": "127.0.0.1",
    "server_port": 8082
}
```

### Arguments

| Flag           | Description                             | Required |
|----------------|-----------------------------------------|----------|
| `-d`, `--dir`  | Path to the directory to watch          | Yes      |
| `-c`, `--config` | Path to the JSON configuration file | No       |

## Usage

### Generating Test Files

Use generate_file.sh to create test files for monitoring.

```bash
./generate_file.sh /path/to/watch 5
```

The above command will generate files in the specified directory every 5 seconds.

### Open a test server

You can use the provided simple TCP server to receive file analysis results.

```bash
python3 -m http.server 8082 --bind 127.0.0.1
```

### Running the Watcher

Run the file system watcher:

```bash
cargo run --release -- -d /path/to/watch
```

OR 

```bash
cargo run --release -- -c configs/config.json
```

