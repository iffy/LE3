# BearCAD issue monitor

Scripts for watching [iffy/BearCAD](https://github.com/iffy/BearCAD) issues labeled `doing` and autonomously fixing them on `devel`.

## Usage

```bash
# Run two poll cycles (verification)
MAX_CYCLES=2 POLL_INTERVAL=5 ./scripts/bearcad-monitor.sh

# Start persistent monitor (single instance via PID lock)
./scripts/bearcad-monitor.sh

# Handle one issue directly
./scripts/bearcad-issue-handler.sh 3
```

Runtime state (logs, checkpoint, PID) is stored in `scripts/monitor-state/` (gitignored).