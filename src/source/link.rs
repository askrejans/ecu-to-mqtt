//! Byte transport to a serial ECU: hardware serial port or a raw TCP socket
//! (ESP32 / Moxa serial bridge). Both implement `AsyncRead + AsyncWrite`.

use anyhow::{Context, Result};
use std::io;
use std::pin::Pin;
use std::task::{Context as TaskContext, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_serial::{SerialPort, SerialPortBuilderExt, SerialPortType, SerialStream};

pub enum Link {
    Serial(SerialStream),
    Tcp(TcpStream),
}

impl Link {
    pub fn open_serial(path: &str, baud: u32) -> Result<Self> {
        let port = tokio_serial::new(path, baud)
            .timeout(Duration::from_millis(500))
            .open_native_async()
            .with_context(|| format!("opening {path} at {baud} baud"))?;
        Ok(Self::Serial(port))
    }

    pub async fn open_tcp(host: &str, port: u16) -> Result<Self> {
        let stream = tokio::time::timeout(Duration::from_secs(5), TcpStream::connect((host, port)))
            .await
            .with_context(|| format!("connecting to {host}:{port} timed out"))?
            .with_context(|| format!("connecting to {host}:{port}"))?;
        stream.set_nodelay(true).ok();
        Ok(Self::Tcp(stream))
    }

    /// Discard unread input (stale bytes from an earlier, timed-out reply).
    pub fn clear_input(&self) {
        if let Self::Serial(port) = self {
            let _ = port.clear(tokio_serial::ClearBuffer::Input);
        }
    }
}

impl AsyncRead for Link {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Serial(s) => Pin::new(s).poll_read(cx, buf),
            Self::Tcp(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Link {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Serial(s) => Pin::new(s).poll_write(cx, buf),
            Self::Tcp(s) => Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Serial(s) => Pin::new(s).poll_flush(cx),
            Self::Tcp(s) => Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Serial(s) => Pin::new(s).poll_shutdown(cx),
            Self::Tcp(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}

/// USB vendors used on ECU boards and their USB-UART bridges.
const KNOWN_VENDORS: &[(u16, &str)] = &[
    (0x2341, "Arduino"),
    (0x2A03, "Arduino"),
    (0x16C0, "Teensy"),
    (0x0483, "STMicroelectronics"),
    (0x10C4, "Silicon Labs CP210x"),
    (0x1A86, "WCH CH34x"),
    (0x0403, "FTDI"),
    (0x067B, "Prolific"),
];

/// Lowest score given to a USB serial port; auto-detection probes only these.
pub const USB_SCORE: u8 = 50;

#[derive(Debug, Clone)]
pub struct Candidate {
    pub path: String,
    pub description: String,
    pub score: u8,
}

/// Serial ports ranked most-likely-ECU first; consoles, Bluetooth and macOS
/// `tty.*` duplicates are skipped.
pub fn candidates() -> Vec<Candidate> {
    let mut out: Vec<Candidate> = tokio_serial::available_ports()
        .unwrap_or_default()
        .into_iter()
        .filter(|p| {
            let n = p.port_name.as_str();
            !(n.starts_with("/dev/tty.") || n.contains("Bluetooth") || n.contains("debug-console"))
        })
        .map(|p| {
            let (description, score) = match &p.port_type {
                SerialPortType::UsbPort(usb) => {
                    let vendor = KNOWN_VENDORS.iter().find(|(v, _)| *v == usb.vid);
                    let product = usb
                        .product
                        .clone()
                        .or(usb.manufacturer.clone())
                        .unwrap_or_default();
                    let lower = product.to_lowercase();
                    let score = if ["ecu", "speeduino", "efi", "megasquirt"]
                        .iter()
                        .any(|k| lower.contains(k))
                    {
                        100
                    } else if vendor.is_some() {
                        80
                    } else {
                        USB_SCORE
                    };
                    let vendor = vendor.map(|(_, n)| format!("[{n}] ")).unwrap_or_default();
                    (
                        format!("USB {:04x}:{:04x} {vendor}{product}", usb.vid, usb.pid)
                            .trim()
                            .to_string(),
                        score,
                    )
                }
                SerialPortType::PciPort => ("PCI serial".into(), 20),
                SerialPortType::BluetoothPort => ("Bluetooth".into(), 5),
                SerialPortType::Unknown => ("serial".into(), 15),
            };
            Candidate {
                path: p.port_name,
                description,
                score,
            }
        })
        .collect();
    out.sort_by(|a, b| b.score.cmp(&a.score).then(a.path.cmp(&b.path)));
    out
}
