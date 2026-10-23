use std::{
    env,
    io::{Read, Write},
    sync::{Mutex, mpsc},
    thread,
};

#[cfg(target_os = "windows")]
use std::{ffi::OsStr, path::Path};

use anyhow::{Context, Result};
use chymeral::transport::SessionEnvironment;
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use winit::event_loop::EventLoopProxy;

use crate::UserEvent;

const OUTPUT_QUEUE_CAPACITY: usize = 64;

pub struct Pty {
    master: Box<dyn MasterPty + Send>,
    writer: Mutex<Box<dyn Write + Send>>,
    child_killer: Box<dyn ChildKiller + Send + Sync>,
    output: mpsc::Receiver<Vec<u8>>,
}

impl Pty {
    pub fn spawn(
        size: PtySize,
        event_proxy: EventLoopProxy<UserEvent>,
        session_environment: &SessionEnvironment,
    ) -> Result<Self> {
        let pty_system = native_pty_system();
        let pair = pty_system.openpty(size).context("could not create a PTY")?;
        let mut command = default_shell_command();
        command.env("TERM", "xterm-256color");
        for (name, value) in session_environment.variables() {
            command.env(name, value);
        }

        let mut child = pair
            .slave
            .spawn_command(command)
            .context("could not start the user shell")?;
        let child_killer = child.clone_killer();
        drop(pair.slave);

        let mut reader = pair
            .master
            .try_clone_reader()
            .context("could not read PTY output")?;
        let writer = pair
            .master
            .take_writer()
            .context("could not write to the PTY")?;
        let (sender, output) = mpsc::sync_channel(OUTPUT_QUEUE_CAPACITY);
        let reader_proxy = event_proxy.clone();

        thread::Builder::new()
            .name("chymeral-pty-reader".to_owned())
            .spawn(move || {
                let mut buffer = [0_u8; 8192];
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(count) => {
                            if sender.send(buffer[..count].to_vec()).is_err() {
                                break;
                            }
                            if reader_proxy.send_event(UserEvent::PtyOutput).is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
            })
            .context("could not start the PTY reader")?;
        let monitor_proxy = event_proxy;
        thread::Builder::new()
            .name("chymeral-pty-monitor".to_owned())
            .spawn(move || {
                let _ = child.wait();
                let _ = monitor_proxy.send_event(UserEvent::PtyClosed);
            })
            .context("could not start the PTY monitor")?;

        Ok(Self {
            master: pair.master,
            writer: Mutex::new(writer),
            child_killer,
            output,
        })
    }

    pub fn write(&self, bytes: &[u8]) -> Result<()> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| anyhow::anyhow!("PTY writer lock poisoned"))?;
        writer
            .write_all(bytes)
            .context("could not write input to the PTY")?;
        writer.flush().context("could not flush PTY input")
    }

    pub fn resize(&self, size: PtySize) -> Result<()> {
        self.master.resize(size).context("could not resize the PTY")
    }

    pub fn drain_output(&self) -> impl Iterator<Item = Vec<u8>> + '_ {
        std::iter::from_fn(|| self.output.try_recv().ok())
    }
}

fn default_shell_command() -> CommandBuilder {
    if let Some(shell) = env::var_os("CHYMERAL_SHELL") {
        #[cfg(target_os = "windows")]
        {
            return windows_shell_command(shell);
        }

        #[cfg(not(target_os = "windows"))]
        {
            return CommandBuilder::new(shell);
        }
    }

    #[cfg(target_os = "windows")]
    {
        CommandBuilder::new(env::var_os("COMSPEC").unwrap_or_else(|| "cmd.exe".into()))
    }

    #[cfg(not(target_os = "windows"))]
    {
        let shell = env::var_os("SHELL").unwrap_or_else(|| "/bin/sh".into());
        let mut command = CommandBuilder::new(shell);
        command.arg("-l");
        command
    }
}

#[cfg(target_os = "windows")]
fn windows_shell_command(shell: std::ffi::OsString) -> CommandBuilder {
    if is_windows_command_script(&shell) {
        let mut command =
            CommandBuilder::new(env::var_os("COMSPEC").unwrap_or_else(|| "cmd.exe".into()));
        command.arg("/d");
        command.arg("/s");
        command.arg("/c");
        command.arg(shell);
        return command;
    }

    CommandBuilder::new(shell)
}

#[cfg(target_os = "windows")]
fn is_windows_command_script(shell: &OsStr) -> bool {
    Path::new(shell)
        .extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("bat")
        })
}

impl Drop for Pty {
    fn drop(&mut self) {
        let _ = self.child_killer.kill();
    }
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use std::ffi::OsStr;

    use super::is_windows_command_script;

    #[test]
    fn recognizes_conventional_windows_command_launchers() {
        assert!(is_windows_command_script(OsStr::new(
            r"C:\apps\foregit-session.cmd"
        )));
        assert!(is_windows_command_script(OsStr::new(
            r"C:\apps\FOREgit.BAT"
        )));
        assert!(!is_windows_command_script(OsStr::new(
            r"C:\apps\foregit.exe"
        )));
    }
}
