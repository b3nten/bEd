use crate::{Client, Inbound};
use bevy::winit::{EventLoopProxyWrapper, WinitUserEvent};
use std::io::{BufRead, Read, Write};

pub(crate) const MAX_LINE: usize = 1024 * 1024;

pub(crate) fn start(client: Client, proxy: Option<&EventLoopProxyWrapper>) {
    let wake = proxy.map(|p| (**p).clone());
    let reader = client.clone();
    let reader_wake = wake.clone();
    std::thread::Builder::new()
        .name("automation-stdin".into())
        .spawn(move || {
            let stdin = std::io::stdin();
            let mut input = stdin.lock();
            loop {
                let mut line = String::new();
                let result = input
                    .by_ref()
                    .take((MAX_LINE + 1) as u64)
                    .read_line(&mut line);
                let message = match result {
                    Ok(0) => Inbound::Eof,
                    Ok(_) if line.len() > MAX_LINE => {
                        // The connection cannot be resynchronized without reading an
                        // unbounded line. Report the error, then close it.
                        let _ = reader
                            .sender
                            .send(Inbound::Invalid("Request exceeds 1 MiB".into()));
                        Inbound::Eof
                    }
                    Ok(_) if line.trim().is_empty() => continue,
                    Ok(_) => match serde_json::from_str(&line) {
                        Ok(value) => Inbound::Request(value),
                        Err(error) => Inbound::Invalid(error.to_string()),
                    },
                    Err(error) => {
                        let _ = reader.sender.send(Inbound::Invalid(error.to_string()));
                        Inbound::Eof
                    }
                };
                let eof = matches!(message, Inbound::Eof);
                if reader.sender.send(message).is_err() {
                    break;
                }
                if let Some(proxy) = &reader_wake {
                    let _ = proxy.send_event(WinitUserEvent::WakeUp);
                }
                if eof {
                    break;
                }
            }
        })
        .expect("Start automation stdin reader");
    std::thread::Builder::new()
        .name("automation-stdout".into())
        .spawn(move || {
            let stdout = std::io::stdout();
            let receiver = client.receiver.lock().unwrap();
            while let Ok(value) = receiver.recv() {
                let mut output = stdout.lock();
                if serde_json::to_writer(&mut output, &value).is_err()
                    || output.write_all(b"\n").is_err()
                    || output.flush().is_err()
                {
                    let _ = client.sender.send(Inbound::Disconnected);
                    if let Some(proxy) = &wake {
                        let _ = proxy.send_event(WinitUserEvent::WakeUp);
                    }
                    break;
                }
                if client
                    .unflushed
                    .fetch_sub(1, std::sync::atomic::Ordering::AcqRel)
                    == 1
                {
                    if let Some(proxy) = &wake {
                        let _ = proxy.send_event(WinitUserEvent::WakeUp);
                    }
                }
            }
        })
        .expect("Start automation stdout writer");
}
