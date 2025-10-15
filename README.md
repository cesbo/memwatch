# memwatch

A simple Linux console utility that runs a given command and monitors its memory usage (RSS and VSZ) including all child processes.

## Build

```bash
cargo build --release
```

## Usage

```bash
memwatch [OPTIONS] -- <command> [args...]
```

## Options

| Option           | Description                           | Default |
| ---------------- | ------------------------------------- | ------- |
| `-i, --interval` | Update interval in milliseconds       | 1000    |

## Output

Line shows elapsed time, RSS, and VSZ:

```
[00:12] RSS: 183.52 MB | VSZ: 224.00 MB
```

## Examples

```
./target/release/memwatch -- python3 - <<'PY'
import time
a = []
for i in range(20):
    a.append(bytearray(10_000_000))  # +10 MB
    time.sleep(1)
PY
```

## License

MIT
