//! Kap byte streams + OS processes (port of `builtins/io_functions.kt` stream
//! surface + `builtins/execprocess.kt`).
//!
//! Kotlin models open files / pipes as `APLBinaryInputStream` /
//! `APLBinaryOutputStream` (`KotlinObjectWrappedValue<ByteProvider|ByteConsumer>`)
//! and spawned commands as `ProcessKapValueWrapper`. The port models them as
//! `APLValue::Stream` / `APLValue::Process` holding `Rc<RefCell<…>>` handles
//! (D1 single-threaded: no threads, no `Send`).
//!
//! Stream semantics mirror the Kotlin originals byte-for-byte where observable:
//! - `readLine`: byte loop to `\n` (the `\n` is consumed, not included; a
//!   trailing `\r` is KEPT); EOF with no collected bytes → nil, else the
//!   partial line (io_functions.kt:218-244).
//! - `lines`: every `\n` emits an entry (even empty ones); a non-empty tail
//!   without trailing `\n` is appended (io_functions.kt:246-280).
//! - `read`: raw bytes as 0-255 numbers, optional limit (io_functions.kt:192-216).

use std::fs::File;
use std::io::{Read, Write};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};

/// Readable end of a Kap stream.
#[derive(Debug)]
pub enum StreamReader {
    File(File),
    /// In-memory bytes (`io2:arrayStream`).
    Memory { data: Vec<u8>, pos: usize },
    ChildOut(ChildStdout),
    ChildErr(ChildStderr),
}

/// Writable end of a Kap stream.
#[derive(Debug)]
pub enum StreamWriter {
    File(File),
    ChildIn(ChildStdin),
}

/// An open Kap byte stream: a reader, a writer, or (transiently) neither.
#[derive(Debug)]
pub struct KapStream {
    pub reader: Option<StreamReader>,
    pub writer: Option<StreamWriter>,
    /// Single-block readahead for `File` / `ChildOut` ends.
    pending: Vec<u8>,
    pending_pos: usize,
    pub closed: bool,
}

impl KapStream {
    pub fn input(reader: StreamReader) -> Self {
        KapStream { reader: Some(reader), writer: None, pending: Vec::new(), pending_pos: 0, closed: false }
    }

    pub fn output(writer: StreamWriter) -> Self {
        KapStream { reader: None, writer: Some(writer), pending: Vec::new(), pending_pos: 0, closed: false }
    }

    pub fn is_input(&self) -> bool {
        self.reader.is_some()
    }

    pub fn display_name(&self) -> &'static str {
        // Kotlin `APLBinaryInputStream.NAME` / `APLBinaryOutputStream.NAME`
        // (io_functions.kt:108-138): used by `formatted()`.
        if self.is_input() {
            "binary input stream"
        } else {
            "binary output stream"
        }
    }

    fn check_open(&self) -> std::io::Result<()> {
        if self.closed {
            Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "stream is closed"))
        } else {
            Ok(())
        }
    }

    fn fill_pending(&mut self) -> std::io::Result<bool> {
        if self.pending_pos < self.pending.len() {
            return Ok(true);
        }
        let mut block = [0u8; 1024];
        let n = match self.reader.as_mut() {
            Some(StreamReader::File(f)) => f.read(&mut block)?,
            Some(StreamReader::ChildOut(c)) => c.read(&mut block)?,
            Some(StreamReader::ChildErr(c)) => c.read(&mut block)?,
            Some(StreamReader::Memory { .. }) => 0,
            None => 0,
        };
        if n == 0 {
            return Ok(false);
        }
        self.pending = block[..n].to_vec();
        self.pending_pos = 0;
        Ok(true)
    }

    /// One raw byte, or `None` at EOF (Kotlin `ByteProvider.readByte`).
    pub fn read_byte(&mut self) -> std::io::Result<Option<u8>> {
        self.check_open()?;
        if let Some(StreamReader::Memory { data, pos }) = self.reader.as_mut() {
            if *pos < data.len() {
                let b = data[*pos];
                *pos += 1;
                return Ok(Some(b));
            }
            return Ok(None);
        }
        if self.reader.is_none() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "stream is not readable",
            ));
        }
        if !self.fill_pending()? {
            return Ok(None);
        }
        let b = self.pending[self.pending_pos];
        self.pending_pos += 1;
        Ok(Some(b))
    }

    /// One `\n`-terminated line (without the `\n`), or `None` on empty EOF
    /// (Kotlin `ReadLineFromStreamFunction`, io_functions.kt:218-244).
    pub fn read_line(&mut self) -> std::io::Result<Option<String>> {
        let mut collected: Vec<u8> = Vec::new();
        loop {
            match self.read_byte()? {
                None => {
                    if collected.is_empty() {
                        return Ok(None);
                    }
                    break;
                }
                Some(b) if b == b'\n' => break,
                Some(b) => collected.push(b),
            }
        }
        Ok(Some(String::from_utf8_lossy(&collected).into_owned()))
    }

    /// All remaining lines (Kotlin `ReadAllLinesFromStreamFunction`,
    /// io_functions.kt:246-280).
    pub fn read_all_lines(&mut self) -> std::io::Result<Vec<String>> {
        let mut out = Vec::new();
        loop {
            match self.read_line()? {
                Some(s) => out.push(s),
                None => break,
            }
        }
        Ok(out)
    }

    /// Raw bytes, up to `limit` (`None` = to EOF). Kotlin
    /// `ReadBytesFromStreamFunction` (io_functions.kt:192-216).
    pub fn read_bytes(&mut self, limit: Option<usize>) -> std::io::Result<Vec<u8>> {
        let mut out = Vec::new();
        loop {
            if let Some(n) = limit {
                if out.len() >= n {
                    break;
                }
            }
            match self.read_byte()? {
                Some(b) => out.push(b),
                None => break,
            }
        }
        Ok(out)
    }

    pub fn write_bytes(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.check_open()?;
        match self.writer.as_mut() {
            Some(StreamWriter::File(f)) => f.write_all(bytes),
            Some(StreamWriter::ChildIn(c)) => c.write_all(bytes),
            None => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "stream is not writable",
            )),
        }
    }

    pub fn flush(&mut self) -> std::io::Result<()> {
        match self.writer.as_mut() {
            Some(StreamWriter::File(f)) => f.flush(),
            Some(StreamWriter::ChildIn(c)) => c.flush(),
            None => Ok(()),
        }
    }

    pub fn close(&mut self) {
        self.closed = true;
        self.reader = None;
        self.writer = None;
        self.pending.clear();
        self.pending_pos = 0;
    }
}

/// A spawned OS process (Kotlin `ProcessKapValueWrapper`, execprocess.kt).
/// stdio pipes are taken at spawn and shared (`Rc`) with every `:stream` call.
#[derive(Debug)]
pub struct KapProcess {
    pub pid: u32,
    child: Child,
    pub stdin: Option<std::rc::Rc<std::cell::RefCell<KapStream>>>,
    pub stdout: Option<std::rc::Rc<std::cell::RefCell<KapStream>>>,
    pub stderr: Option<std::rc::Rc<std::cell::RefCell<KapStream>>>,
}

impl KapProcess {
    pub fn spawn(cmd: &str, args: &[String]) -> std::io::Result<Self> {
        let mut child = Command::new(cmd)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let pid = child.id();
        let mk = |s: KapStream| std::rc::Rc::new(std::cell::RefCell::new(s));
        let stdin = child
            .stdin
            .take()
            .map(|s| mk(KapStream::output(StreamWriter::ChildIn(s))));
        let stdout = child
            .stdout
            .take()
            .map(|s| mk(KapStream::input(StreamReader::ChildOut(s))));
        let stderr = child
            .stderr
            .take()
            .map(|s| mk(KapStream::input(StreamReader::ChildErr(s))));
        Ok(KapProcess { pid, child, stdin, stdout, stderr })
    }

    /// Wait up to `timeout_ms` for exit (`-1` = indefinitely). Returns the exit
    /// code, or `None` on timeout (Kotlin `ProcessWaitMethod`: `nil`).
    /// `0` = single non-blocking poll (the `execLs` test's usage).
    pub fn wait_timeout(&mut self, timeout_ms: f64) -> std::io::Result<Option<i32>> {
        if timeout_ms < 0.0 {
            return Ok(self.child.wait()?.code());
        }
        let deadline = std::time::Instant::now()
            + std::time::Duration::from_secs_f64(timeout_ms.max(0.0) / 1000.0);
        loop {
            if let Some(status) = self.child.try_wait()? {
                return Ok(status.code());
            }
            if std::time::Instant::now() >= deadline {
                return Ok(None);
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
}
