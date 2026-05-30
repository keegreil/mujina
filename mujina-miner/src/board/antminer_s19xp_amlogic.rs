//! Antminer S19XP Amlogic control board support.
//!
//! This is a local Linux board, not a USB-hotplug board. It is intended for
//! non-invasive bringup on a stock/LuxOS Amlogic control board after the stock
//! miner process has released the ASIC UART.

use std::{
    env,
    fmt::Display,
    fs,
    fs::{File, OpenOptions},
    io::Write,
    os::fd::AsRawFd,
    path::PathBuf,
    str::FromStr,
    time::Duration,
};

use anyhow::{Context as _, Result, bail};
use async_trait::async_trait;
use futures::SinkExt;
use tokio::{sync::watch, time};
use tokio_stream::StreamExt;
use tokio_util::{
    codec::{FramedRead, FramedWrite},
    sync::CancellationToken,
};

use crate::{
    api_client::types::{BoardTelemetry, ThreadTelemetry},
    asic::{
        ChipInfo,
        bm13xx::{
            self, BM13xxProtocol, ProfiledFrameCodec, ResponseProfile,
            protocol::{BaudRate, ChipProfile, Command, Register},
            thread::{BM13xxThread, BM13xxThreadConfig},
        },
        hash_thread::{AsicEnable, BoardPeripherals, HashThread, ThreadRemovalSignal},
    },
    board::{BackplaneConnector, BoardInfo},
    tracing::prelude::*,
    transport::serial::{SerialControl, SerialReader, SerialStream, SerialWriter},
    types::HashRate,
};

const BOARD_ID: &str = "s19xp-amlogic";
const DEFAULT_EXPECTED_CHIPS: usize = 110;
const DEFAULT_FREQUENCY_MHZ: f32 = 110.0;
const DEFAULT_BAUD_RATE: u32 = 115_200;
const DEFAULT_STARTUP_BAUD_RATE: u32 = 115_200;
const DEFAULT_NONCE_RANGE: u32 = 0x0000_1446;
const DEFAULT_PS_ENABLE_GPIO: u32 = 437;
const DEFAULT_PS_ENABLE_DELAY_MS: u64 = 2_000;
const DEFAULT_CHAIN_RESET_GPIO: u32 = 456;
const DEFAULT_APW12_I2C_DEVICE: &str = "/dev/i2c-1";
const DEFAULT_APW12_I2C_ADDRESS: u16 = 0x10;
const DEFAULT_APW12_TARGET_VOLTAGE: f32 = 12.0;
const MIN_APW12_TARGET_VOLTAGE: f32 = 11.9;
const MAX_APW12_TARGET_VOLTAGE: f32 = 15.0;
const I2C_SLAVE_IOCTL: nix::libc::c_ulong = 0x0703;

/// Create the local S19XP board from environment configuration.
pub async fn create_from_env() -> Result<BackplaneConnector> {
    let config = S19xpConfig::from_env()?;

    info!(
        tty = %config.asic_tty,
        startup_baud = config.startup_baud_rate,
        baud = config.baud_rate,
        expected_chips = config.expected_chips,
        frequency_mhz = config.frequency_mhz,
        nonce_range = format_args!("0x{:08x}", config.nonce_range),
        ps_enable_gpio = ?config.ps_enable_gpio,
        ps_enable_active_high = config.ps_enable_active_high,
        apw12_target_voltage = config.apw12_target_voltage,
        chain_reset_gpio = ?config.chain_reset_gpio,
        reset_gpio = ?config.reset_gpio,
        probe_only = config.probe_only,
        "Opening S19XP Amlogic ASIC UART"
    );

    let mut ps_enable = match config.ps_enable_gpio {
        Some(gpio) => Some(SysfsOutputLine::new(
            gpio,
            config.ps_enable_active_high,
            "PS enable",
        )?),
        None => None,
    };

    let mut chain_reset = match config.chain_reset_gpio {
        Some(gpio) => Some(SysfsOutputLine::new(gpio, true, "chain reset release")?),
        None => None,
    };

    if config.apw12_startup {
        run_s19xp_apw12_startup_sequence(&config, ps_enable.as_mut(), chain_reset.as_mut()).await?;
    } else if let Some(ps_enable) = ps_enable.as_mut() {
        info!(
            gpio = ps_enable.gpio,
            active_high = ps_enable.active_high,
            delay_ms = config.ps_enable_delay_ms,
            "Enabling S19XP hashboard power rail"
        );
        ps_enable.set_enabled(true)?;
        time::sleep(Duration::from_millis(config.ps_enable_delay_ms)).await;
    } else {
        warn!("No S19XP PS-enable GPIO configured; assuming hashboard power is already on");
    }

    let data_stream = SerialStream::new(&config.asic_tty, config.startup_baud_rate)
        .with_context(|| format!("failed to open ASIC UART {}", config.asic_tty))?;
    let (data_reader, data_writer, data_control) = data_stream.split();
    let mut data_reader = FramedRead::new(
        data_reader,
        ProfiledFrameCodec::new(ResponseProfile::BM1366),
    );
    let mut data_writer = FramedWrite::new(
        data_writer,
        ProfiledFrameCodec::new(ResponseProfile::BM1366),
    );

    let mut reset = match config.reset_gpio {
        Some(gpio) => Some(SysfsResetLine::new(gpio, config.reset_active_low)?),
        None => None,
    };

    if let Some(reset) = reset.as_mut() {
        reset.disable().await?;
        time::sleep(Duration::from_millis(200)).await;
        reset.enable().await?;
        time::sleep(Duration::from_millis(500)).await;
    } else {
        warn!("No S19XP reset GPIO configured; assuming the hashboard is already released");
    }

    let chip_infos = discover_bm1366_chips(
        &mut data_reader,
        &mut data_writer,
        config.expected_chips,
        Duration::from_secs(2),
    )
    .await?;

    let discovered = chip_infos.len();
    if discovered != config.expected_chips {
        warn!(
            discovered,
            expected = config.expected_chips,
            "BM1366 chip count differs from configured expectation"
        );
    }

    if let Some(reset) = reset.as_mut() {
        reset.disable().await?;
    }

    if config.startup_baud_rate != config.baud_rate {
        set_chain_baudrate(&mut data_writer, &data_control, config.baud_rate).await?;
    }

    if config.probe_only {
        if let Some(ps_enable) = ps_enable.as_mut() {
            ps_enable.set_enabled(false)?;
        }

        info!(
            discovered,
            expected = config.expected_chips,
            "S19XP Amlogic probe completed; no hash threads will be registered"
        );

        let board_name = BOARD_ID.to_string();
        let info = BoardInfo {
            model: "Antminer S19XP Amlogic".to_string(),
            firmware_version: Some("local-linux-probe".to_string()),
            serial_number: None,
        };
        let initial_telemetry = BoardTelemetry {
            name: board_name,
            model: info.model.clone(),
            serial: None,
            threads: Vec::new(),
            ..Default::default()
        };
        let (_telemetry_tx, telemetry_rx) = watch::channel(initial_telemetry);

        return Ok(BackplaneConnector {
            info,
            threads: Vec::new(),
            telemetry_rx,
            shutdown: None,
        });
    }

    let (thread_shutdown_tx, thread_shutdown_rx) = watch::channel(ThreadRemovalSignal::Running);
    let thread_name = format!("S19XP-Amlogic-BM1366-{discovered}ch");
    let thread_config = BM13xxThreadConfig {
        profile: ChipProfile::BM1366,
        chain_length: discovered,
        target_frequency_mhz: config.frequency_mhz,
        nonce_range: Some(bm13xx::protocol::NonceRangeConfig::from_register_value(
            config.nonce_range,
        )),
        hashrate_estimate: estimate_s19xp_hashrate(config.frequency_mhz, discovered),
        asic_difficulty: None,
    };

    let peripherals = BoardPeripherals {
        asic_enable: reset.map(|line| Box::new(line) as Box<dyn AsicEnable>),
        voltage_regulator: None,
    };

    let thread = BM13xxThread::new_with_config(
        thread_name.clone(),
        data_reader,
        data_writer,
        peripherals,
        thread_shutdown_rx,
        thread_config,
    );
    let threads: Vec<Box<dyn HashThread>> = vec![Box::new(thread)];

    let board_name = BOARD_ID.to_string();
    let info = BoardInfo {
        model: "Antminer S19XP Amlogic".to_string(),
        firmware_version: Some("local-linux".to_string()),
        serial_number: None,
    };
    let initial_telemetry = BoardTelemetry {
        name: board_name,
        model: info.model.clone(),
        serial: None,
        threads: vec![ThreadTelemetry {
            name: thread_name,
            hashrate: 0,
            is_active: false,
        }],
        ..Default::default()
    };
    let (telemetry_tx, telemetry_rx) = watch::channel(initial_telemetry);
    let cancel = CancellationToken::new();
    let monitor_handle = tokio::spawn(run_static_telemetry(telemetry_tx, cancel.clone()));

    let shutdown = Box::pin(async move {
        let _ = thread_shutdown_tx.send(ThreadRemovalSignal::Shutdown);
        cancel.cancel();
        let _ = monitor_handle.await;
        if let Some(ps_enable) = ps_enable.as_mut()
            && let Err(e) = ps_enable.set_enabled(false)
        {
            warn!(error = %e, "Failed to disable S19XP hashboard power rail");
        }
    });

    info!(
        discovered,
        frequency_mhz = config.frequency_mhz,
        "S19XP Amlogic board initialized"
    );

    Ok(BackplaneConnector {
        info,
        threads,
        telemetry_rx,
        shutdown: Some(shutdown),
    })
}

async fn run_static_telemetry(
    _telemetry_tx: watch::Sender<BoardTelemetry>,
    cancel: CancellationToken,
) {
    cancel.cancelled().await;
}

async fn run_s19xp_apw12_startup_sequence(
    config: &S19xpConfig,
    ps_enable: Option<&mut SysfsOutputLine>,
    chain_reset: Option<&mut SysfsOutputLine>,
) -> Result<()> {
    let mut apw12 = S19xpApw12::open(&config.apw12_i2c_device, config.apw12_i2c_address)?;
    let mut ps_enable = ps_enable;
    let mut chain_reset = chain_reset;

    info!(
        i2c = %config.apw12_i2c_device,
        address = format_args!("0x{:02x}", config.apw12_i2c_address),
        "Starting S19XP APW12 power sequence"
    );

    let _ = apw12.write(&[0x00]);
    time::sleep(Duration::from_millis(500)).await;
    apw12.command(&[0x02])?;
    time::sleep(Duration::from_millis(800)).await;
    apw12.command(&[0x01])?;
    time::sleep(Duration::from_millis(400)).await;

    if let Some(line) = chain_reset.as_mut() {
        line.set_enabled(false)?;
    }

    time::sleep(Duration::from_millis(1_000)).await;
    apw12.command(&[0x06, 0x40, 0x20])?;
    time::sleep(Duration::from_millis(900)).await;
    apw12.command(&[0x81, 0x00, 0x00])?;
    time::sleep(Duration::from_millis(1_400)).await;

    if let Some(line) = ps_enable.as_mut() {
        line.set_enabled(true)?;
    }

    let voltage_dac = S19xpApw12::encode_voltage_to_dac(config.apw12_target_voltage)?;
    info!(
        target_voltage = config.apw12_target_voltage,
        dac = voltage_dac,
        "Setting S19XP APW12 target voltage"
    );
    apw12.command(&[0x83, voltage_dac, 0x00])?;
    time::sleep(Duration::from_millis(600)).await;
    apw12.command(&[0x03])?;
    time::sleep(Duration::from_millis(config.ps_enable_delay_ms)).await;

    if let Some(line) = chain_reset.as_mut() {
        line.set_enabled(false)?;
        time::sleep(Duration::from_millis(200)).await;
        line.set_enabled(true)?;
        time::sleep(Duration::from_millis(100)).await;
        line.set_enabled(false)?;
        time::sleep(Duration::from_millis(100)).await;
        line.set_enabled(true)?;
        time::sleep(Duration::from_millis(1_000)).await;
    }

    Ok(())
}

async fn set_chain_baudrate(
    writer: &mut FramedWrite<SerialWriter, ProfiledFrameCodec>,
    control: &SerialControl,
    baud_rate: u32,
) -> Result<()> {
    let baud = match baud_rate {
        115_200 => BaudRate::Baud115200,
        1_000_000 => BaudRate::Baud1M,
        3_000_000 => BaudRate::Baud3M,
        other => {
            bail!("unsupported S19XP runtime baud rate {other}; use 115200, 1000000, or 3000000")
        }
    };

    info!(baud = baud_rate, "Switching S19XP ASIC UART baud");
    writer
        .send(BM13xxProtocol::new().set_baudrate(baud))
        .await
        .context("failed to send BM1366 baud-rate command")?;
    time::sleep(Duration::from_millis(100)).await;
    control
        .set_baud_rate(baud_rate)
        .context("failed to switch host ASIC UART baud")?;
    time::sleep(Duration::from_millis(100)).await;
    Ok(())
}

async fn discover_bm1366_chips(
    reader: &mut FramedRead<SerialReader, ProfiledFrameCodec>,
    writer: &mut FramedWrite<SerialWriter, ProfiledFrameCodec>,
    expected_chips: usize,
    timeout: Duration,
) -> Result<Vec<ChipInfo>> {
    for _ in 1..=3 {
        writer
            .send(Command::WriteRegister {
                broadcast: true,
                chip_address: 0x00,
                register: Register::VersionMask(bm13xx::protocol::VersionMask::full_rolling()),
            })
            .await
            .context("failed to send version mask during discovery")?;
        time::sleep(Duration::from_millis(5)).await;
    }

    writer
        .send(BM13xxProtocol::discover_chips())
        .await
        .context("failed to send BM1366 chip discovery command")?;

    let mut chip_infos = Vec::with_capacity(expected_chips);
    let deadline = time::Instant::now() + timeout;

    while time::Instant::now() < deadline && chip_infos.len() < expected_chips {
        tokio::select! {
            response = reader.next() => {
                match response {
                    Some(Ok(bm13xx::Response::ReadRegister {
                        chip_address: _,
                        register: Register::ChipId { chip_type, core_count, address },
                    })) if chip_type == bm13xx::protocol::ChipType::BM1366 => {
                        let chip_id = chip_type.id_bytes();
                        chip_infos.push(ChipInfo {
                            chip_id,
                            core_count: core_count.into(),
                            address,
                            supports_version_rolling: true,
                        });
                    }
                    Some(Ok(other)) => {
                        trace!(response = ?other, "Ignoring non-BM1366 discovery response");
                    }
                    Some(Err(e)) => {
                        warn!(error = %e, "Error while discovering BM1366 chips");
                    }
                    None => break,
                }
            }
            _ = time::sleep_until(deadline) => break,
        }
    }

    if chip_infos.is_empty() {
        bail!("no BM1366 chips discovered on S19XP chain");
    }

    Ok(chip_infos)
}

fn estimate_s19xp_hashrate(frequency_mhz: f32, chip_count: usize) -> HashRate {
    // Conservative planning estimate: roughly 85 GH/s per BM1366 chip at
    // 110 MHz, scaled linearly for the configured test frequency.
    let th = chip_count as f64 * 0.085 * (frequency_mhz as f64 / DEFAULT_FREQUENCY_MHZ as f64);
    HashRate::from_terahashes(th)
}

#[derive(Debug)]
struct S19xpConfig {
    asic_tty: String,
    startup_baud_rate: u32,
    baud_rate: u32,
    expected_chips: usize,
    frequency_mhz: f32,
    nonce_range: u32,
    apw12_startup: bool,
    apw12_i2c_device: String,
    apw12_i2c_address: u16,
    apw12_target_voltage: f32,
    ps_enable_gpio: Option<u32>,
    ps_enable_active_high: bool,
    ps_enable_delay_ms: u64,
    chain_reset_gpio: Option<u32>,
    reset_gpio: Option<u32>,
    reset_active_low: bool,
    probe_only: bool,
}

impl S19xpConfig {
    fn from_env() -> Result<Self> {
        let asic_tty = env::var("MUJINA_S19XP_ASIC_TTY")
            .context("MUJINA_S19XP_ASIC_TTY must point to the ASIC UART device")?;
        let baud_rate = env_parse("MUJINA_S19XP_BAUD", DEFAULT_BAUD_RATE)?;
        let startup_baud_rate = env_parse("MUJINA_S19XP_STARTUP_BAUD", DEFAULT_STARTUP_BAUD_RATE)?;
        let expected_chips = env_parse("MUJINA_S19XP_EXPECTED_CHIPS", DEFAULT_EXPECTED_CHIPS)?;
        let frequency_mhz = env_parse("MUJINA_S19XP_FREQUENCY_MHZ", DEFAULT_FREQUENCY_MHZ)?;
        let nonce_range = match env::var("MUJINA_S19XP_NONCE_RANGE") {
            Ok(raw) => parse_u32_auto(&raw).context("invalid MUJINA_S19XP_NONCE_RANGE")?,
            Err(_) => DEFAULT_NONCE_RANGE,
        };
        let apw12_startup = env_bool_alias(
            "MUJINA_S19XP_APW12_STARTUP",
            "MUJINA_S19XP_PIC_STARTUP",
            true,
        )?;
        let apw12_i2c_device = env::var("MUJINA_S19XP_APW12_I2C_DEVICE")
            .or_else(|_| env::var("MUJINA_S19XP_PIC_I2C_DEVICE"))
            .unwrap_or_else(|_| DEFAULT_APW12_I2C_DEVICE.to_string());
        let apw12_i2c_address = match env::var("MUJINA_S19XP_APW12_I2C_ADDRESS")
            .or_else(|_| env::var("MUJINA_S19XP_PIC_I2C_ADDRESS"))
        {
            Ok(raw) => parse_u16_auto(&raw)
                .context("invalid MUJINA_S19XP_APW12_I2C_ADDRESS / MUJINA_S19XP_PIC_I2C_ADDRESS")?,
            Err(_) => DEFAULT_APW12_I2C_ADDRESS,
        };
        let apw12_target_voltage = env_parse(
            "MUJINA_S19XP_APW12_TARGET_VOLTAGE",
            DEFAULT_APW12_TARGET_VOLTAGE,
        )?;
        let ps_enable_gpio =
            env_optional_u32("MUJINA_S19XP_PS_ENABLE_GPIO", Some(DEFAULT_PS_ENABLE_GPIO))?;
        let ps_enable_active_high = env_bool("MUJINA_S19XP_PS_ENABLE_ACTIVE_HIGH", false)?;
        let ps_enable_delay_ms = env_parse(
            "MUJINA_S19XP_PS_ENABLE_DELAY_MS",
            DEFAULT_PS_ENABLE_DELAY_MS,
        )?;
        let chain_reset_gpio = env_optional_u32(
            "MUJINA_S19XP_CHAIN_RESET_GPIO",
            Some(DEFAULT_CHAIN_RESET_GPIO),
        )?;
        let reset_gpio = match env::var("MUJINA_S19XP_RESET_GPIO") {
            Ok(raw) if !raw.trim().is_empty() => {
                Some(parse_u32_auto(&raw).context("invalid MUJINA_S19XP_RESET_GPIO")?)
            }
            _ => None,
        };
        let reset_active_low = env_bool("MUJINA_S19XP_RESET_ACTIVE_LOW", true)?;
        let probe_only = env_bool("MUJINA_S19XP_PROBE_ONLY", false)?;

        if !(50.0..=200.0).contains(&frequency_mhz) {
            bail!("MUJINA_S19XP_FREQUENCY_MHZ must stay in the low bringup range 50..=200 MHz");
        }
        if !(MIN_APW12_TARGET_VOLTAGE..=MAX_APW12_TARGET_VOLTAGE).contains(&apw12_target_voltage) {
            bail!(
                "MUJINA_S19XP_APW12_TARGET_VOLTAGE must be in the safe bringup range {:.1}..={:.1} V",
                MIN_APW12_TARGET_VOLTAGE,
                MAX_APW12_TARGET_VOLTAGE
            );
        }

        Ok(Self {
            asic_tty,
            startup_baud_rate,
            baud_rate,
            expected_chips,
            frequency_mhz,
            nonce_range,
            apw12_startup,
            apw12_i2c_device,
            apw12_i2c_address,
            apw12_target_voltage,
            ps_enable_gpio,
            ps_enable_active_high,
            ps_enable_delay_ms,
            chain_reset_gpio,
            reset_gpio,
            reset_active_low,
            probe_only,
        })
    }
}

fn env_parse<T>(key: &str, default: T) -> Result<T>
where
    T: FromStr,
    T::Err: Display,
{
    match env::var(key) {
        Ok(raw) => raw
            .parse::<T>()
            .map_err(|e| anyhow::anyhow!("invalid {key}: {e}")),
        Err(_) => Ok(default),
    }
}

fn env_optional_u32(key: &str, default: Option<u32>) -> Result<Option<u32>> {
    let Ok(raw) = env::var(key) else {
        return Ok(default);
    };

    let trimmed = raw.trim();
    if trimmed.is_empty()
        || matches!(
            trimmed.to_ascii_lowercase().as_str(),
            "none" | "disable" | "disabled"
        )
    {
        return Ok(None);
    }

    parse_u32_auto(trimmed)
        .map(Some)
        .with_context(|| format!("invalid {key}"))
}

fn parse_u32_auto(raw: &str) -> Result<u32> {
    let trimmed = raw.trim();
    if let Some(hex) = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
    {
        Ok(u32::from_str_radix(hex, 16)?)
    } else {
        Ok(trimmed.parse()?)
    }
}

fn parse_u16_auto(raw: &str) -> Result<u16> {
    let value = parse_u32_auto(raw)?;
    Ok(u16::try_from(value)?)
}

fn env_bool(key: &str, default: bool) -> Result<bool> {
    let Ok(raw) = env::var(key) else {
        return Ok(default);
    };

    parse_bool(&raw)
        .with_context(|| format!("invalid {key}: expected true/false/1/0/yes/no/on/off"))
}

fn env_bool_alias(primary: &str, legacy: &str, default: bool) -> Result<bool> {
    match env::var(primary) {
        Ok(raw) => parse_bool(&raw)
            .with_context(|| format!("invalid {primary}: expected true/false/1/0/yes/no/on/off")),
        Err(_) => env_bool(legacy, default),
    }
}

fn parse_bool(raw: &str) -> Result<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        other => bail!("invalid boolean value {other:?}"),
    }
}

struct SysfsOutputLine {
    gpio: u32,
    active_high: bool,
    value_path: PathBuf,
    label: &'static str,
}

impl SysfsOutputLine {
    fn new(gpio: u32, active_high: bool, label: &'static str) -> Result<Self> {
        let gpio_dir = PathBuf::from(format!("/sys/class/gpio/gpio{gpio}"));
        if !gpio_dir.exists() {
            fs::write("/sys/class/gpio/export", gpio.to_string())
                .with_context(|| format!("failed to export GPIO {gpio} for {label}"))?;
            std::thread::sleep(Duration::from_millis(100));
        }

        let direction_path = gpio_dir.join("direction");
        fs::write(&direction_path, "out")
            .with_context(|| format!("failed to set GPIO {gpio} direction for {label}"))?;

        let value_path = gpio_dir.join("value");
        if !value_path.exists() {
            bail!("GPIO {gpio} value path does not exist after export for {label}");
        }

        Ok(Self {
            gpio,
            active_high,
            value_path,
            label,
        })
    }

    fn value_for(&self, enabled: bool) -> &'static str {
        match (self.active_high, enabled) {
            (true, true) | (false, false) => "1",
            (true, false) | (false, true) => "0",
        }
    }

    fn set_enabled(&self, enabled: bool) -> Result<()> {
        fs::write(&self.value_path, self.value_for(enabled)).with_context(|| {
            format!(
                "failed to write GPIO {} value for {}",
                self.gpio, self.label
            )
        })
    }
}

struct S19xpApw12 {
    file: File,
}

impl S19xpApw12 {
    fn open(path: &str, address: u16) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .with_context(|| format!("failed to open S19XP APW12 I2C device {path}"))?;

        let ret = unsafe {
            nix::libc::ioctl(
                file.as_raw_fd(),
                I2C_SLAVE_IOCTL,
                address as nix::libc::c_ulong,
            )
        };
        if ret < 0 {
            bail!(
                "failed to select S19XP APW12 I2C address 0x{address:02x} on {path}: {}",
                std::io::Error::last_os_error()
            );
        }

        Ok(Self { file })
    }

    fn write(&mut self, bytes: &[u8]) -> Result<()> {
        self.file
            .write_all(bytes)
            .context("failed to write S19XP APW12 I2C packet")
    }

    fn command(&mut self, payload: &[u8]) -> Result<()> {
        let packet = Self::packet(payload)?;
        self.write(&packet)
    }

    fn encode_voltage_to_dac(voltage: f32) -> Result<u8> {
        if !(MIN_APW12_TARGET_VOLTAGE..=MAX_APW12_TARGET_VOLTAGE).contains(&voltage) {
            bail!(
                "S19XP APW12 target voltage must be in range {:.1}..={:.1} V",
                MIN_APW12_TARGET_VOLTAGE,
                MAX_APW12_TARGET_VOLTAGE
            );
        }

        let dac = ((voltage - 15.092) / -0.013).round();
        Ok(dac.clamp(0.0, 255.0) as u8)
    }

    fn packet(payload: &[u8]) -> Result<Vec<u8>> {
        let len = u8::try_from(payload.len() + 3).context("S19XP APW12 payload too long")?;
        let checksum = payload
            .iter()
            .fold(len, |acc, byte| acc.wrapping_add(*byte));
        let mut packet = Vec::with_capacity(payload.len() + 5);
        packet.extend_from_slice(&[0x11, 0x55, 0xaa, len]);
        packet.extend_from_slice(payload);
        packet.extend_from_slice(&[checksum, 0x00]);
        Ok(packet)
    }
}

struct SysfsResetLine {
    gpio: u32,
    active_low: bool,
    value_path: PathBuf,
}

impl SysfsResetLine {
    fn new(gpio: u32, active_low: bool) -> Result<Self> {
        let gpio_dir = PathBuf::from(format!("/sys/class/gpio/gpio{gpio}"));
        if !gpio_dir.exists() {
            fs::write("/sys/class/gpio/export", gpio.to_string())
                .with_context(|| format!("failed to export GPIO {gpio}"))?;
            std::thread::sleep(Duration::from_millis(100));
        }

        let direction_path = gpio_dir.join("direction");
        fs::write(&direction_path, "out")
            .with_context(|| format!("failed to set GPIO {gpio} direction"))?;

        let value_path = gpio_dir.join("value");
        if !value_path.exists() {
            bail!("GPIO {gpio} value path does not exist after export");
        }

        Ok(Self {
            gpio,
            active_low,
            value_path,
        })
    }

    fn write_enabled(&self, enabled: bool) -> Result<()> {
        let value = match (self.active_low, enabled) {
            (true, true) => "1",
            (true, false) => "0",
            (false, true) => "0",
            (false, false) => "1",
        };
        fs::write(&self.value_path, value)
            .with_context(|| format!("failed to write GPIO {} value", self.gpio))
    }
}

#[async_trait]
impl AsicEnable for SysfsResetLine {
    async fn enable(&mut self) -> Result<()> {
        self.write_enabled(true)
    }

    async fn disable(&mut self) -> Result<()> {
        self.write_enabled(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bool_accepts_common_env_values() {
        assert!(parse_bool("1").unwrap());
        assert!(parse_bool("true").unwrap());
        assert!(parse_bool("YES").unwrap());
        assert!(!parse_bool("0").unwrap());
        assert!(!parse_bool("false").unwrap());
        assert!(!parse_bool("off").unwrap());
        assert!(parse_bool("maybe").is_err());
    }

    #[test]
    fn sysfs_output_line_maps_active_low_power_enable() {
        let line = SysfsOutputLine {
            gpio: 437,
            active_high: false,
            value_path: PathBuf::from("/dev/null"),
            label: "test",
        };

        assert_eq!(line.value_for(true), "0");
        assert_eq!(line.value_for(false), "1");
    }

    #[test]
    fn apw12_packet_matches_luxos_trace_shape() {
        assert_eq!(
            S19xpApw12::packet(&[0x81, 0x00, 0x00]).unwrap(),
            vec![0x11, 0x55, 0xaa, 0x06, 0x81, 0x00, 0x00, 0x87, 0x00]
        );
    }

    #[test]
    fn apw12_voltage_encoding_targets_luxos_low_voltage_range() {
        assert_eq!(S19xpApw12::encode_voltage_to_dac(12.0).unwrap(), 238);
        assert_eq!(S19xpApw12::encode_voltage_to_dac(15.0).unwrap(), 7);
        assert!(S19xpApw12::encode_voltage_to_dac(11.8).is_err());
    }
}
