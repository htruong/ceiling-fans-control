//! Pure-Rust CC1101 driver for the Pi-native SPI transport.
//!
//! Drives the CC1101 in packet/FIFO mode with sync, preamble, whitening, and
//! CRC all disabled, so the chip clocks our pre-encoded chip stream straight
//! out of the TX FIFO at the symbol rate. No user-space GPIO bit-banging
//! required — SPI only. The Pi Zero W is comfortably fast enough for this.
//!
//! See protocol.txt for the on-air format; see the README "Pi-native"
//! section for wiring + bring-up. Register values were lifted from TI's
//! SmartRF Studio 300 MHz OOK reference profile, with FREQ and PATABLE
//! programmed at init() time so frequency/power stay tunable from config.
//!
//! Linux-only: spidev is a Linux ioctl wrapper. main.rs cfg-gates the
//! `Transport::Spi` code path on non-Linux targets.

use std::io;
use std::thread;
use std::time::Duration;

use spidev::{SpiModeFlags, Spidev, SpidevOptions, SpidevTransfer};

// ── SPI header byte flags ────────────────────────────────────────────────────
const WRITE_SINGLE: u8 = 0x00;
const WRITE_BURST:  u8 = 0x40;
const READ_BURST:   u8 = 0xC0;

// ── Command strobes (single-byte writes to header) ───────────────────────────
const SRES:  u8 = 0x30;
const SCAL:  u8 = 0x33;
const STX:   u8 = 0x35;
const SIDLE: u8 = 0x36;
const SFTX:  u8 = 0x3B;

// ── Registers we touch by name ───────────────────────────────────────────────
const PKTLEN:    u8 = 0x06;
const PATABLE:   u8 = 0x3E;
const TX_FIFO:   u8 = 0x3F;
// Status registers (read-only; MUST be read with READ_BURST flag set, else
// the chip returns the status byte instead of the register value — catches
// everyone the first time).
const PARTNUM:   u8 = 0x30;
const VERSION:   u8 = 0x31;
const MARCSTATE: u8 = 0x35;

const FXOSC_HZ: u64 = 26_000_000;

/// Static register profile written verbatim at init time. FREQ (0x0D..0x0F)
/// and PATABLE are NOT included here — they're computed/programmed in
/// `init()` from caller-supplied values.
const INIT_REGS: &[(u8, u8)] = &[
    (0x00, 0x29), // IOCFG2  = CHIP_RDY (unused but a safe default)
    (0x02, 0x06), // IOCFG0  = sync-asserts / pkt-deasserts (also unused)
    (0x03, 0x47), // FIFOTHR = default TX/RX FIFO threshold
    (0x04, 0x00), // SYNC1   — sync disabled, value moot
    (0x05, 0x00), // SYNC0
    (0x07, 0x04), // PKTCTRL1 = APPEND_STATUS, no addr check
    (0x08, 0x00), // PKTCTRL0 = fixed length, FIFO mode, no whitening, no CRC
    (0x0B, 0x06), // FSCTRL1 = IF freq (TI 300 MHz OOK rec.)
    (0x0C, 0x00), // FSCTRL0
    (0x10, 0x86), // MDMCFG4 = CHANBW 203 kHz (high nibble), DRATE_E 6 (low)
    (0x11, 0xE4), // MDMCFG3 = DRATE_M 228 → ≈3001 baud (333 µs/chip)
    (0x12, 0x30), // MDMCFG2 = OOK/ASK, no Manchester, SYNC_MODE = none
    (0x13, 0x22), // MDMCFG1
    (0x14, 0xF8), // MDMCFG0
    (0x15, 0x00), // DEVIATN (unused for OOK)
    (0x17, 0x30), // MCSM1   = CCA off; after-TX → IDLE; after-RX → IDLE
    (0x18, 0x18), // MCSM0   = auto-calibrate on IDLE→TX/RX transition
    (0x19, 0x16), // FOCCFG
    (0x20, 0xFB), // WORCTRL
    (0x22, 0x11), // FREND0  = PATABLE index 1 is the "carrier on" level
    (0x23, 0xE9), // FSCAL3
    (0x24, 0x2A), // FSCAL2
    (0x25, 0x00), // FSCAL1
    (0x26, 0x1F), // FSCAL0
    (0x2C, 0x81), // TEST2
    (0x2D, 0x35), // TEST1
    (0x2E, 0x09), // TEST0
];

pub struct Cc1101 {
    spi: Spidev,
}

impl Cc1101 {
    /// Open `/dev/spidev<bus>.<cs>` and return a configured handle. The
    /// kernel SPI driver manages CS for us, so no GPIO is needed.
    pub fn open(path: &str) -> io::Result<Self> {
        let mut spi = Spidev::open(path)?;
        spi.configure(
            &SpidevOptions::new()
                .bits_per_word(8)
                .max_speed_hz(5_000_000) // CC1101 datasheet caps at 6.5 MHz
                .mode(SpiModeFlags::SPI_MODE_0)
                .build(),
        )?;
        Ok(Self { spi })
    }

    fn xfer(&mut self, tx: &[u8], rx: &mut [u8]) -> io::Result<()> {
        let mut t = SpidevTransfer::read_write(tx, rx);
        self.spi.transfer(&mut t)
    }

    fn strobe(&mut self, cmd: u8) -> io::Result<u8> {
        let tx = [cmd];
        let mut rx = [0u8];
        self.xfer(&tx, &mut rx)?;
        Ok(rx[0])
    }

    fn write_reg(&mut self, addr: u8, val: u8) -> io::Result<()> {
        let tx = [addr | WRITE_SINGLE, val];
        let mut rx = [0u8; 2];
        self.xfer(&tx, &mut rx)
    }

    fn write_burst(&mut self, addr: u8, data: &[u8]) -> io::Result<()> {
        let mut tx = Vec::with_capacity(1 + data.len());
        tx.push(addr | WRITE_BURST);
        tx.extend_from_slice(data);
        let mut rx = vec![0u8; tx.len()];
        self.xfer(&tx, &mut rx)
    }

    fn read_status(&mut self, addr: u8) -> io::Result<u8> {
        let tx = [addr | READ_BURST, 0x00];
        let mut rx = [0u8; 2];
        self.xfer(&tx, &mut rx)?;
        Ok(rx[1])
    }

    /// Reset the chip, write the static register profile, program FREQ from
    /// `frequency_hz` and PATABLE[1] from `power`, then calibrate. Errors if
    /// PARTNUM/VERSION don't look like a real CC1101 (catches "is it even
    /// plugged in" before transmit-time).
    pub fn init(&mut self, frequency_hz: u32, power: u8) -> io::Result<()> {
        self.strobe(SRES)?;
        thread::sleep(Duration::from_millis(5));

        for &(a, v) in INIT_REGS {
            self.write_reg(a, v)?;
        }

        // FREQ = freq * 2^16 / Fxosc  (datasheet §21)
        let freq = ((frequency_hz as u64) << 16) / FXOSC_HZ;
        self.write_reg(0x0D, ((freq >> 16) & 0xFF) as u8)?;
        self.write_reg(0x0E, ((freq >> 8) & 0xFF) as u8)?;
        self.write_reg(0x0F, (freq & 0xFF) as u8)?;

        // PATABLE[0] = off (0 mW), PATABLE[1] = on (caller-supplied byte;
        // 0xC0 is the standard "max power, 315 MHz band" entry, ≈+11 dBm).
        // FREND0 (written above) tells the modulator to use index 1 as the
        // carrier-on level, so OOK chips toggle between the two.
        self.write_burst(PATABLE, &[0x00, power])?;

        self.strobe(SIDLE)?;
        self.strobe(SCAL)?;
        thread::sleep(Duration::from_micros(800));

        let partnum = self.read_status(PARTNUM)?;
        let version = self.read_status(VERSION)?;
        // PARTNUM is always 0x00. VERSION is typically 0x04 or 0x14; the
        // exact value varies by die rev. 0x00 / 0xFF mean MISO is dead.
        if partnum != 0x00 || version == 0x00 || version == 0xFF {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!(
                    "CC1101 not responding: PARTNUM=0x{:02X} VERSION=0x{:02X} (expected PARTNUM=0x00, VERSION≈0x04/0x14)",
                    partnum, version,
                ),
            ));
        }
        Ok(())
    }

    /// Encode `bits` ('0'/'1' string of any length) into the 3-chip "10b" form
    /// the fans expect, append 30 silent chips (~10 ms inter-burst pause), and
    /// pack MSB-first into bytes. The CC1101 clocks these out at the symbol
    /// rate from INIT_REGS (~3000 baud → 333 µs/chip). Bit-length agnostic so
    /// other OOK fans with different frame lengths work without changes.
    pub fn encode(bits: &str) -> Vec<u8> {
        let mut chips = String::with_capacity(bits.len() * 3 + 32);
        for b in bits.chars() {
            chips.push('1');
            chips.push('0');
            chips.push(b);
        }
        for _ in 0..30 {
            chips.push('0');
        }
        // Pad to a whole byte; trailing bits are zeros, i.e. more silence.
        while chips.len() % 8 != 0 {
            chips.push('0');
        }
        chips
            .as_bytes()
            .chunks(8)
            .map(|c| {
                c.iter()
                    .enumerate()
                    .map(|(i, &ch)| ((ch - b'0') as u8) << (7 - i))
                    .sum()
            })
            .collect()
    }

    /// Transmit `bits` `repeat` times. Blocks until each frame fully drains
    /// from the FIFO and the chip returns to IDLE (~37 ms per 25-bit frame
    /// at 3000 baud). Caller is responsible for serialising concurrent
    /// transmits (a Mutex around the Cc1101 handle suffices).
    pub fn transmit(&mut self, bits: &str, repeat: u32) -> io::Result<()> {
        let payload = Self::encode(bits);
        // Rewrite PKTLEN per call so a daemon driving fans of different
        // protocols transparently works without re-init.
        self.write_reg(PKTLEN, payload.len() as u8)?;

        for _ in 0..repeat {
            self.strobe(SIDLE)?;
            self.strobe(SFTX)?;
            self.write_burst(TX_FIFO, &payload)?;
            self.strobe(STX)?;

            // MARCSTATE: 0x01 = IDLE. TX states are 0x13..0x16; MCSM1's
            // TXOFF_MODE = 0 sends us straight back to IDLE on FIFO drain.
            loop {
                let s = self.read_status(MARCSTATE)? & 0x1F;
                if s == 0x01 {
                    break;
                }
                thread::sleep(Duration::from_micros(500));
            }
        }
        self.strobe(SIDLE)?;
        Ok(())
    }
}
