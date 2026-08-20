# garld

**G**itHub **A**ction **R**unner **L**ocal **D**ashboard — see what your self-hosted GitHub Actions runners are doing on the machine that hosts them.

`garld` finds every runner installed on the host, tells you whether each one is idle or building, **which repository and job** it's building, how long it's been going, and what it's costing the machine. It ships as one binary with four faces: a CLI, a live terminal view, a desktop window, a floating widget, and a menu-bar/tray item.

Everything is read locally — the runners' own install directories, their `_diag` logs, and the process table. **No GitHub token, no network calls, nothing to configure.**

```
$ garld
garld · ci-mac-01 · Darwin 26.6.2 (arm64, 10 cores) · up 12h06m
cpu [||              ]  11.5%   mem [|||||||||||     ] 11.4G/16.0G (71%)   swap 3.56G/5.00G   load 6.10 9.90 9.93   730 procs

RUNNERS  1 busy · 0 idle · 0 offline
● busy    ci-mac-01 · acme-corp · v2.336.0 · pool Default
  job  web-app · release / ios · 14m02s
  cost 118% cpu · 1.10G · 24 procs · listener pid 1624 up 12h02m
  last ✓ web-app · release / macos             16m14s
       ✓ api · release / ios                    7m27s
       ! api · release / macos                 33m27s
       ✓ mobile · release / ios                 7m57s
       70% pass over 10 recorded job(s)

PROCESSES  6 runner process(es), by cpu
    PID ROLE     USER           CPU%   MEM%      RSS TIME       NAME
   2081 worker   ci              0.9    0.5    85.8M 0:30.76    Runner.Worker
  84340 job      ci             94.2    6.3    1.01G 0:03.50    xcodebuild
   1600 svc      ci              0.0    0.0    32.0K 0:00.01    bash
   1624 listener ci              0.0    0.1    21.0M 0:13.47    Runner.Listener
```

## Why

A self-hosted runner is a black box on your own hardware. GitHub's web UI tells you a job is "running on a self-hosted runner"; it doesn't tell you that the runner is thrashing on swap, that a previous job leaked a process tree that's still burning CPU, or which of your six repositories is the one currently pinning every core.

`garld` answers those from the host's point of view.

## What it shows

**Per runner** — name, organisation, runner version, pool, service identifier, and whether it's `busy`, `idle` (listener up, no job) or `offline` (installed, listener not running).

**The running job** — repository, workflow and job name, elapsed time, and the workspace it checked out into.

**What the job actually costs** — CPU, memory and process count for the worker *and every process it spawned*. This matters: `Runner.Worker` itself sits near zero while `xcodebuild`, `cargo`, `gradle` or `pytest` do the work, so per-job cost is summed over the whole subtree.

**Job history** — recent jobs with result, duration and repository, reconstructed from the listener log, plus a pass rate.

**Runner processes, tagged** — the process table labels every runner-owned process by role: `svc` (the `run.sh`/`runsvc.sh` wrapper), `listener` (`Runner.Listener`), `worker` (`Runner.Worker`) and `job` (anything a worker spawned).

**Host metrics** — CPU (total and per core), memory, swap, load average, uptime, process and thread counts.

## Install

### macOS

```bash
brew tap kekko7072/garld
brew install garld          # garld and garld-gui on your PATH, built from source
brew install --cask garld   # garld.app in /Applications
```

The formula is the one to reach for if you want the command line: it compiles
locally, so nothing is quarantined and macOS asks nothing of you. Add the cask
too if you want garld in Launchpad. They install different things and do not
conflict.

Or download `garld-<version>-macos.dmg` from [Releases][releases] and drag
**garld** to Applications.

**First launch:** garld is signed ad-hoc, not notarised by Apple, so macOS asks
you to confirm. Right-click **garld** in `/Applications`, choose **Open**, and
confirm; later launches are normal.

Notarisation needs a paid Apple Developer account, which this project doesn't
have. If you'd rather avoid the prompt entirely, `brew install garld` or
`cargo install` builds from source and Gatekeeper never gets involved.

### Windows

Download `garld-<version>-windows-x86_64.zip` from [Releases][releases] and unpack it.

- `garld-gui.exe` — double-click for the dashboard (opens no console window)
- `garld.exe` — the CLI, for a terminal

### Linux

```bash
sudo dpkg -i garld_<version>_amd64.deb        # Debian, Ubuntu
```

Or unpack `garld-<version>-linux-<arch>.tar.gz` and run `./install.sh`, which
installs into `~/.local` without root (`./install.sh --system` for `/usr/local`).
Both add a desktop entry, so **garld** appears in your application menu.

### From source

Requires Rust 1.95 or newer.

```bash
cargo install --git https://github.com/kekko7072/garld
```

Or clone and build:

```bash
git clone https://github.com/kekko7072/garld
cd garld
cargo build --release
./target/release/garld
```

This produces two binaries: `garld` (console) and `garld-gui` (windowed).

#### Linux build dependencies

The window and widget need the usual X11/Wayland development packages, and the
tray item additionally needs GTK 3 and `libxdo`:

```bash
# Debian / Ubuntu
sudo apt install build-essential libgtk-3-dev libxdo-dev \
                 libxkbcommon-dev libwayland-dev libx11-dev \
                 libxcursor-dev libxrandr-dev libxi-dev libgl1-mesa-dev

# Fedora
sudo dnf install gtk3-devel libxdo-devel libxkbcommon-devel wayland-devel
```

Don't want the graphical surfaces at all? Build a smaller, dependency-free CLI:

```bash
cargo build --release --no-default-features                  # CLI only
cargo build --release --no-default-features --features gui   # no tray
```

macOS and Windows need no extra packages.

#### Building the installers yourself

```bash
packaging/macos/bundle.sh --universal --dmg   # dist/garld.app + dist/*.dmg
packaging/linux/tarball.sh                    # dist/*.tar.gz
cargo deb                                     # dist/*.deb
python3 packaging/icon/make_icons.py packaging/icon   # regenerate the icons
```

[releases]: https://github.com/kekko7072/garld/releases

## Usage

```
garld                    one-shot dashboard: runners, jobs, host metrics, processes
garld watch              the same, refreshed on an interval
garld runners            just the runners and their jobs
garld ps                 just the process table
garld info               host and runner detail, one field per line
garld gui                the dashboard window
garld widget             a compact always-on-top desktop widget
garld tray               menu-bar item (macOS) / notification area (Windows, Linux)
```

Useful flags:

| Flag | Meaning |
| --- | --- |
| `-a`, `--all-processes` | show every process, not just the runner's own (the default is runner-only) |
| `-s`, `--sort <KEY>` | order by `cpu`, `mem`, `pid`, `name`, `time`, `io` or `user` |
| `-n`, `--limit <N>` | cap the number of rows; `0` for all |
| `-f`, `--filter <TEXT>` | match name, command line, exe path, or an exact pid |
| `-u`, `--user <NAME>` | only processes owned by this user |
| `-w`, `--wide` | full command lines instead of process names |
| `--history <N>` | recent jobs to list per runner; `0` hides history |
| `--no-processes` | runners and metrics only |
| `--json` | machine-readable output of everything |
| `-i`, `--interval <SECS>` | `watch` refresh period (default 2) |
| `--count <N>` | `watch` stops after N frames — handy in scripts |
| `--runner-dir <PATH>` | inspect a runner in a non-standard location (repeatable) |

Scripting example — alert when any job runs longer than an hour:

```bash
garld --json | jq -r '
  .runners.runners[]
  | select(.current_job != null)
  | select((now - .current_job.started) > 3600)
  | "\(.current_job.repository) \(.current_job.workflow)/\(.current_job.job) is overdue"'
```

## The graphical surfaces

**`garld gui`** — runners down the left with live job and history, a sortable process table in the middle, click any row for full detail, and CPU/memory/job-load sparklines across the top.

**`garld widget`** — a small borderless, translucent, always-on-top panel showing runner state, the current repository and job, elapsed time and host load. Drag it anywhere by its background. On macOS it runs as an accessory app, so it takes no Dock tile.

**`garld tray`** — on macOS the menu bar shows live text (`1/1 busy · 37% cpu`) beside a status icon that fills with CPU load and takes its colour from runner state. Windows and Linux show the icon plus the same text as a tooltip. Its menu carries runner counts, the current job, host metrics and the busiest process, and can open the window or the widget.

## How runners are found

1. **Running processes.** Any live `Runner.Listener`, `Runner.Worker` or `run.sh`/`runsvc.sh` reveals its install root, so an active runner is always found wherever it lives.
2. **Conventional locations.** Directories named `actions-runner*`, `github-runner*` or `runner*` under your home directory, `~/Developer`, `/opt`, `/usr/local`, `/var/lib`, or `C:\` — so stopped runners appear as `offline` rather than vanishing.
3. **Explicit paths.** `--runner-dir <PATH>`, repeatable, or a path-separated list in `GARLD_RUNNER_DIRS`.

A directory counts as a runner install when it contains a `.runner` file.

Repository attribution comes from the runner's own bookkeeping: `.runner` for identity, `_work/_PipelineMapping/<owner>/<repo>/PipelineFolder.json` for the repository behind each checkout directory, and each job's `Worker_*.log` to tie a specific job to a specific repository. Job history and results come from `Running job:` and `completed with result:` lines in the listener log.

## What garld does not read

Runner install directories hold credentials. `garld` **never opens**:

- `.credentials` and `.credentials_rsaparams` — the runner's OAuth token and private key
- `.env` and `.path` — routinely used to carry secrets into jobs

Only `.runner` (identity), `.service` (service name), `_diag/*.log` and `_work/_PipelineMapping/**/PipelineFolder.json` are read. This is enforced by a test that greps the source for reads of the forbidden names, so it can't quietly regress.

`garld` is read-only. It never signals, kills, starts or stops a runner or any other process, and it makes no network connections.

## Platform notes

| | macOS | Windows | Linux |
| --- | --- | --- | --- |
| CLI, watch, window, widget | ✅ | ✅ | ✅ |
| Menu bar / tray | ✅ menu bar with live text | ✅ icon + tooltip | ✅ via libappindicator |
| Per-process thread counts | not reported by the OS | ✅ | ✅ |
| Load average | ✅ | not available on Windows | ✅ |

Columns the platform can't fill are dropped rather than padded with dashes.

## Contributing

Issues and pull requests are welcome. Before opening a PR:

```bash
cargo fmt
cargo clippy --all-features -- -D warnings
cargo test
```

If you're adding a parser for runner log output, add a test with a literal log excerpt — the existing ones in `src/runner.rs` show the pattern. Log formats vary between runner versions, and a sample in a test is the only thing that keeps that honest.

## Releasing

Tagging triggers [`release.yml`](.github/workflows/release.yml), which builds the
universal macOS app and disk image, the Windows executables, and the Linux
tarball and `.deb`, then publishes them with checksums.

```bash
git tag v0.1.0 && git push origin v0.1.0
```

Once the release is up, refresh the Homebrew tap:

```bash
packaging/homebrew/sync-tap.sh 0.1.0 ../homebrew-garld
```

That fills the version and checksums into the formula and cask templates in
`packaging/homebrew/` and writes them into the tap checkout to commit.

## Licence

MIT — see [LICENSE](LICENSE).
