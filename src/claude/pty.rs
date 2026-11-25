//! Claude in a pseudo-terminal: its output feeds a vt100 screen deck draws,
//! deck writes the keys back.

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::mpsc::Sender;
use tui_term::vt100;

const SCROLLBACK: usize = 5000;

pub struct Pty {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    pub parser: vt100::Parser,
    size: (u16, u16),
}

fn size(rows: u16, cols: u16) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

impl Pty {
    /// Starts `program args` in `cwd`; its output arrives on `output`, and an
    /// empty chunk when it closed the terminal.
    pub fn spawn(
        program: &str,
        args: &[String],
        cwd: &Path,
        (rows, cols): (u16, u16),
        output: Sender<Vec<u8>>,
    ) -> Result<Pty, String> {
        let pair = native_pty_system()
            .openpty(size(rows, cols))
            .map_err(|error| error.to_string())?;
        let mut command = CommandBuilder::new(program);
        command.args(args);
        command.cwd(cwd);
        command.env("TERM", "xterm-256color");
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|error| format!("{program}: {error}"))?;
        drop(pair.slave);
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| error.to_string())?;
        std::thread::spawn(move || {
            let mut buffer = [0u8; 16 * 1024];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) | Err(_) => {
                        let _ = output.send(Vec::new());
                        return;
                    }
                    Ok(n) => {
                        if output.send(buffer[..n].to_vec()).is_err() {
                            return;
                        }
                    }
                }
            }
        });
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| error.to_string())?;
        Ok(Pty {
            master: pair.master,
            writer,
            child,
            parser: vt100::Parser::new(rows, cols, SCROLLBACK),
            size: (rows, cols),
        })
    }

    pub fn write(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let _ = self.writer.write_all(bytes);
        let _ = self.writer.flush();
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        if (rows, cols) == self.size || rows == 0 || cols == 0 {
            return;
        }
        self.size = (rows, cols);
        let _ = self.master.resize(size(rows, cols));
        self.parser.screen_mut().set_size(rows, cols);
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.process_id()
    }

    /// Its exit code once it ended.
    pub fn exited(&mut self) -> Option<u32> {
        self.child
            .try_wait()
            .ok()
            .flatten()
            .map(|status| status.exit_code())
    }

    pub fn kill(&mut self) {
        let _ = self.child.kill();
    }

    /// Scrolls the view into the scrollback; 0 is the live screen.
    pub fn scroll(&mut self, by: isize) {
        let screen = self.parser.screen_mut();
        let now = screen.scrollback() as isize;
        screen.set_scrollback((now + by).max(0) as usize);
    }
}
