# S19XP Amlogic Bringup

This path is for non-invasive bringup on an Antminer S19XP Amlogic control
board while another firmware image, such as LuxOS, remains installed.

The S19XP board support is opt-in at daemon startup:

```bash
MUJINA_USB_DISABLE=1 \
MUJINA_LOCAL_BOARD=s19xp-amlogic \
MUJINA_S19XP_ASIC_TTY=/dev/ttyS1 \
MUJINA_POOL_URL=stratum+tcp://example.pool:3333 \
MUJINA_POOL_USER=worker.name \
MUJINA_POOL_PASS=x \
RUST_LOG=mujina_miner=debug \
mujina-minerd
```

Replace `/dev/ttyS1` if the ASIC UART differs on the target control board. Do
not start Mujina against the ASIC UART until the stock miner process has been
stopped and released the device.

## Defaults

- Board: one S19XP hashboard
- Chip profile: BM1366
- Expected chain length: `110`
- Target frequency: `110 MHz`
- Nonce range: `0x00001446` (LuxOS S19XP value)
- Startup/runtime UART baud: `115200`
- Fan control: none

The control board does not manage the fan in this setup. Cooling is external
via the fixed-speed duct fan, so the default frequency intentionally stays at
the known-stable low LuxOS value.

The default baud rate is intentionally conservative. The cold-start path powers
the board through the APW12/GPIO sequence and discovers the chain at the BM1366
default `115200` baud. A `3000000` runtime baud handoff works when LuxOS has
already initialized the chain, but the cold-start baud switch still needs more
work.

## Environment

- `MUJINA_S19XP_ASIC_TTY`: required ASIC UART path.
- `MUJINA_S19XP_PROBE_ONLY`: optional, default `false`. When true, Mujina
  opens the ASIC UART and enumerates BM1366 chips, then registers no hash
  threads. This is the safest first-stage live test.
- `MUJINA_S19XP_FREQUENCY_MHZ`: optional, default `110`, accepted range
  `50..=200` for bringup.
- `MUJINA_S19XP_EXPECTED_CHIPS`: optional, default `110`.
- `MUJINA_S19XP_NONCE_RANGE`: optional, default `0x00001446`.
- `MUJINA_S19XP_STARTUP_BAUD`: optional, default `115200`.
- `MUJINA_S19XP_BAUD`: optional, default `115200`.
- `MUJINA_S19XP_APW12_STARTUP`: optional, default `true`. Legacy
  `MUJINA_S19XP_PIC_STARTUP` is still accepted.
- `MUJINA_S19XP_APW12_I2C_DEVICE`: optional, default `/dev/i2c-1`. Legacy
  `MUJINA_S19XP_PIC_I2C_DEVICE` is still accepted.
- `MUJINA_S19XP_APW12_I2C_ADDRESS`: optional, default `0x10`. Legacy
  `MUJINA_S19XP_PIC_I2C_ADDRESS` is still accepted.
- `MUJINA_S19XP_APW12_TARGET_VOLTAGE`: optional, default `12.0`. The accepted
  static bringup range is `11.9..=15.0` V. The low default avoids the 15 V
  APW12 startup setting that wastes power and heat at low test frequencies.
- `MUJINA_S19XP_PS_ENABLE_GPIO`: optional, default `437`, active-low.
- `MUJINA_S19XP_CHAIN_RESET_GPIO`: optional, default `456`, final high.
- `MUJINA_S19XP_RESET_GPIO`: optional Linux sysfs GPIO number for hashboard
  reset/enable.
- `MUJINA_S19XP_RESET_ACTIVE_LOW`: optional, default `true`.

## First Probe

Before configuring `MUJINA_S19XP_ASIC_TTY` and reset GPIO, collect these from
LuxOS without stopping services or writing flash:

```bash
uname -a
cat /proc/cmdline
tr -d '\0' </proc/device-tree/model; echo
ls -l /dev/tty* /dev/i2c-* /dev/gpio* 2>/dev/null
ps w
dmesg | grep -Ei 'uart|tty|gpio|i2c|hashboard|asic'
```

Once the device mapping is known, stop only the stock miner service/process,
then start Mujina with the env vars above.

The repository also includes a target-side read-only collector:

```bash
./scripts/s19xp-preflight.sh
```

It writes a timestamped report under `/tmp/mujina-s19xp-preflight` and does not
stop services, write flash, or touch ASIC control lines.

## Non-Destructive Test Mode

The current approach is a temporary handoff, not an installer:

- It does not flash firmware.
- It does not replace `/luxminer`.
- It does not edit `/etc/init.d`, `/etc/rc*.d`, or `/config`.
- It does not persist across reboot unless someone later adds init-script or
  config changes.

LuxOS and Mujina cannot run the ASIC at the same time because LuxOS holds
`/dev/ttyS1`. For a real mining test, LuxOS' miner/watchdog must be stopped for
the duration of the timed run, then restarted afterward.

For a first-stage UART/chip discovery test without registering hash threads:

```bash
MUJINA_USB_DISABLE=1 \
MUJINA_LOCAL_BOARD=s19xp-amlogic \
MUJINA_S19XP_ASIC_TTY=/dev/ttyS1 \
MUJINA_S19XP_PROBE_ONLY=1 \
RUST_LOG=mujina_miner=debug,mujina_miner::asic::bm13xx=trace \
timeout 30s ./mujina-minerd
```

This still needs LuxOS to release `/dev/ttyS1`, but it should not start hashing.

## Timed Handoff Script

`scripts/s19xp-run-mujina.sh` is intended to be copied beside the target
`mujina-minerd` binary on the miner. It refuses to run unless explicitly armed:

```bash
CONFIRM_S19XP_RUN=yes \
STOP_LUXOS=yes \
RUN_SECONDS=120 \
./scripts/s19xp-run-mujina.sh
```

Defaults used by the script:

- `MUJINA_LOCAL_BOARD=s19xp-amlogic`
- `MUJINA_USB_DISABLE=1`
- `MUJINA_S19XP_ASIC_TTY=/dev/ttyS1`
- `MUJINA_S19XP_FREQUENCY_MHZ=110`
- `MUJINA_S19XP_EXPECTED_CHIPS=110`
- `MUJINA_S19XP_STARTUP_BAUD=115200`
- `MUJINA_S19XP_BAUD=115200`
- `MUJINA_S19XP_APW12_STARTUP=true`
- `MUJINA_S19XP_APW12_I2C_DEVICE=/dev/i2c-1`
- `MUJINA_S19XP_APW12_I2C_ADDRESS=0x10`
- `MUJINA_S19XP_APW12_TARGET_VOLTAGE=12.0`
- `MUJINA_S19XP_PS_ENABLE_GPIO=437`
- `MUJINA_S19XP_PS_ENABLE_ACTIVE_HIGH=false`
- `MUJINA_S19XP_PS_ENABLE_DELAY_MS=2000`
- `MUJINA_S19XP_CHAIN_RESET_GPIO=456`
- `MUJINA_POOL_URL=stratum+tcp://pool.256foundation.org:3333`
- `MUJINA_POOL_USER=AgentP.Mujina_on_S19XP`
- `MUJINA_POOL_PASS=x`
- `RUN_SECONDS=120`

The script starts LuxOS again after Mujina exits when `STOP_LUXOS=yes` is used.
The companion `scripts/s19xp-rollback-luxos.sh` can be run manually to terminate
Mujina and restart LuxOS.

## Manual Soak Run

`scripts/s19xp-soak.sh` is the preferred longer-run harness. It runs Mujina
until it is interrupted, writes a dedicated timestamped run directory, samples
basic telemetry every 30 seconds, and attempts to restart LuxOS on exit.

From the unpacked bundle on the miner:

```bash
cd /tmp/mujina-test/s19xp-amlogic
CONFIRM_S19XP_RUN=yes ./scripts/s19xp-soak.sh
```

The soak script defaults to `MUJINA_S19XP_FREQUENCY_MHZ=100` and
`MUJINA_S19XP_APW12_TARGET_VOLTAGE=12.0` for lower heat during unattended-style
observation. Override either explicitly if needed:

```bash
MUJINA_S19XP_FREQUENCY_MHZ=110 \
MUJINA_S19XP_APW12_TARGET_VOLTAGE=12.0 \
CONFIRM_S19XP_RUN=yes \
./scripts/s19xp-soak.sh
```

Useful optional overrides:

```bash
SNAPSHOT_INTERVAL=10 CONFIRM_S19XP_RUN=yes ./scripts/s19xp-soak.sh
STOP_METHOD=kill CONFIRM_S19XP_RUN=yes ./scripts/s19xp-soak.sh
```

The default `STOP_METHOD=graceful` intentionally starts from the harder state:
LuxOS is allowed to shut down cleanly, then Mujina must power the hashboard back
up through GPIO437 and the APW12 command path. `STOP_METHOD=kill` is available
for comparison with the older handoff behavior.

Stop the soak with `Ctrl-C` in the SSH session running the script. The trap will
terminate Mujina, take a final snapshot, and restart LuxOS. If the SSH session
dies or you want to force a rollback from another shell:

```bash
cd /tmp/mujina-test/s19xp-amlogic
./scripts/s19xp-rollback-luxos.sh
```

To package the newest soak logs on the miner:

```bash
cd /tmp/mujina-test/s19xp-amlogic
./scripts/s19xp-collect-soak.sh
```

That prints a `/tmp/mujina-s19xp-soak-<timestamp>.tar.gz` path and a matching
`.sha256` file. Pull them back to the development machine with:

```bash
scp root@192.168.1.200:/tmp/mujina-s19xp-soak-*.tar.gz .
scp root@192.168.1.200:/tmp/mujina-s19xp-soak-*.tar.gz.sha256 .
```

## Deployment Prep

Build a local-Linux Amlogic binary without USB discovery:

```bash
rustup target add aarch64-unknown-linux-gnu
cargo build -p mujina-miner \
  --bin mujina-minerd \
  --target aarch64-unknown-linux-gnu \
  --release \
  --no-default-features
```

This build mode avoids the USB/udev discovery path and uses Rustls instead of
OpenSSL. The host still needs an aarch64 Linux C toolchain, usually:

```bash
sudo apt-get install gcc-aarch64-linux-gnu
```

For a static binary that runs on LuxOS without a dynamic loader:

```bash
RUSTFLAGS='-C target-feature=+crt-static' cargo build -p mujina-miner \
  --bin mujina-minerd \
  --target aarch64-unknown-linux-gnu \
  --release \
  --no-default-features
```

After the binary exists, package the volatile test bundle:

```bash
./scripts/s19xp-package.sh
```

Keep the unpacked test bundle in volatile storage on the miner:

```bash
scp target/s19xp-amlogic.tar.gz root@192.168.1.200:/tmp/
ssh root@192.168.1.200
rm -rf /tmp/mujina-test
mkdir -p /tmp/mujina-test
tar -xzf /tmp/s19xp-amlogic.tar.gz -C /tmp/mujina-test
cd /tmp/mujina-test/s19xp-amlogic
```

Running from `/tmp` or `/mnt/ramdisk` keeps the test non-persistent. A reboot
returns the unit to LuxOS startup behavior.

## First Live Test Notes

Test date: 2026-05-29.

- Kernel: Linux 4.9.241, aarch64, device-tree model `Amlogic`.
- ASIC UART: `/dev/ttyS1`; `/luxminer` holds it open while running.
- LuxOS sets `/dev/ttyS1` to `3000000` baud after startup.
- LuxOS profile: `110MHz`.
- Observed single-board status: hashboard id `2`, `110MHz`, roughly
  `10.7-10.8 TH/s`, about `11.9-12.0 V`.
- Exported boot-script GPIO hints: `437` is labeled `PS enable`, `438` red LED,
  `453` green LED, `447-450` fan tach/IRQ.
- `GPIO437` was observed as `out 0` while LuxOS was hashing, so Mujina treats
  the APW12 / hashboard power-enable line as active-low by default and asserts
  it before chip discovery.
- GPIO diffing across LuxOS running/stopped showed `GPIO456` changes from `1`
  while running to `0` after clean stop. Mujina now uses it as the chain reset /
  release line during cold startup.
- Kernel I2C/GPIO tracing of LuxOS startup showed an APW12 command sequence on
  `/dev/i2c-1` address `0x10`, register `0x11`; Mujina now replays that
  sequence before chip discovery. Earlier notes called this a PIC sequence, but
  Skot's S19j Pro branch indicates this path is PSU/APW12 control.
- APW12 voltage command `0x83 0x00 0x00` maps to the high-voltage startup
  setting. Mujina now encodes `MUJINA_S19XP_APW12_TARGET_VOLTAGE`, default
  `12.0` V, for low-power static tests.
- Initial hashing found valid chip nonces but filtered every candidate because
  BM1366 nonce responses were decoded big-endian. The S19XP capture showed the
  nonce field must be decoded little-endian.
- After the endian fix, a 60 second volatile run found 48 chip shares, submitted
  accepted pool shares to `pool.256foundation.org:3333`, saw 0 rejects and 0
  errors, and reported about `6.38 TH/s` at shutdown. LuxOS was restarted after
  the timed run.
- After adding APW12/GPIO cold startup, a probe-only run from graceful LuxOS stop
  discovered all 110 chips, and a 60 second cold-start hashing run at `115200`
  found 48 chip shares with 24 accepted pool shares, 0 rejects, and about
  `11.35 TH/s` by shutdown.
