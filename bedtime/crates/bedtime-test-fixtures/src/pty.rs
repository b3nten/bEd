use bedtime_terminal::terminal_pty::MAX_QUEUED_WRITE_BYTES;
use std::{
    fs,
    io::{self, IsTerminal, Read, Write},
    thread,
    time::Duration,
};
pub fn child(scenario: &str) {
    raw_child_stdio();
    #[cfg(unix)]
    if scenario == "stubborn" || scenario == "stubborn_foreground" {
        unsafe {
            libc::signal(libc::SIGHUP, libc::SIG_IGN);
            libc::signal(libc::SIGTERM, libc::SIG_IGN);
        }
    }
    println!("PTY_READY");
    io::stdout().flush().unwrap();
    if scenario == "stubborn_foreground" {
        fs::write(
            std::env::var("BED_FG_PID_FILE").unwrap(),
            std::process::id().to_string(),
        )
        .unwrap();
        thread::sleep(Duration::from_secs(10));
        return;
    }
    if scenario == "pressure_reply" {
        thread::sleep(Duration::from_millis(200));
        print!("\x1b[2J\x1b[H\x1b[6n");
        io::stdout().flush().unwrap();
        let mut input = vec![0; MAX_QUEUED_WRITE_BYTES];
        io::stdin().read_exact(&mut input).unwrap();
        assert!(input.iter().all(|byte| *byte == b'x'));
        println!("PRESSURE_REPLY:{}", hexadecimal(&read_until(b'R')));
        return;
    }
    if scenario == "sync_stop" {
        print!("\x1b[?2026hSYNCHRONIZED_FINAL_STDOUT");
        io::stdout().flush().unwrap();
        fs::write(std::env::var("BED_SYNC_FLAG").unwrap(), b"written").unwrap();
        thread::sleep(Duration::from_secs(10));
        return;
    }
    if scenario == "background_flood" {
        io::stdout()
            .write_all(&vec![b'X'; 2 * 1024 * 1024])
            .unwrap();
        println!("\nBACKGROUND_FLOOD_END");
        return;
    }
    if scenario == "silent" || scenario == "stubborn" {
        thread::sleep(Duration::from_secs(10));
        return;
    }
    loop {
        let line = read_until(b'\n');
        let command = String::from_utf8_lossy(&line);
        match command.trim_end_matches('\n') {
            "REPORT" => {
                println!(
                    "TTY:{}{}{}",
                    u8::from(io::stdin().is_terminal()),
                    u8::from(io::stdout().is_terminal()),
                    u8::from(io::stderr().is_terminal())
                );
                println!("ENV_TERM:{}", std::env::var("TERM").unwrap());
                println!(
                    "ENV_PROGRAM:{}",
                    std::env::var("TERM_PROGRAM").unwrap_or_default()
                );
                println!(
                    "ENV_CUSTOM:{}",
                    std::env::var("BED_PTY_TEST").unwrap_or_default()
                );
                println!(
                    "CWD:{}",
                    std::env::current_dir()
                        .unwrap()
                        .canonicalize()
                        .unwrap()
                        .display()
                );
                println!(
                    "ARGV:{}",
                    serde_json::to_string(&std::env::args().skip(3).collect::<Vec<_>>()).unwrap()
                );
                println!("REPORT_END");
            }
            "SIZE" => child_size(),
            "REMOVED_ENV" => {
                println!("PATH_ABSENT:{}", std::env::var_os("PATH").is_none());
                println!("TERM_ABSENT:{}", std::env::var_os("TERM").is_none());
                println!(
                    "OVERRIDE:{}",
                    std::env::var("BED_PTY_TEST").unwrap_or_default()
                );
                println!("REMOVED_ENV_END");
            }
            "HEX" => println!("HEX:{}", hexadecimal(&read_until(b'\n'))),
            "REPLY" => {
                print!("\x1b[2J\x1b[H\x1b[6n");
                io::stdout().flush().unwrap();
                println!("REPLY:{}", hexadecimal(&read_until(b'R')));
            }
            "FLOOD" => {
                let bytes = vec![b'X'; 2 * 1024 * 1024];
                io::stdout().write_all(&bytes).unwrap();
                println!("\nFLOOD_END");
            }
            "QUIT" => {
                println!("FINAL_STDOUT");
                eprintln!("FINAL_STDERR");
                io::stdout().flush().unwrap();
                io::stderr().flush().unwrap();
                std::process::exit(7);
            }
            value if value.starts_with("ECHO:") => println!("ECHOED:{}", &value[5..]),
            value => panic!("Unexpected PTY command {value:?}"),
        }
        io::stdout().flush().unwrap();
    }
}
fn read_until(last: u8) -> Vec<u8> {
    let mut bytes = Vec::new();
    loop {
        let mut byte = [0];
        io::stdin().read_exact(&mut byte).unwrap();
        bytes.push(byte[0]);
        if byte[0] == last {
            return bytes;
        }
        assert!(
            bytes.len() < 4096,
            "PTY input protocol exceeded fixture limit"
        );
    }
}
fn hexadecimal(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
#[cfg(unix)]
fn raw_child_stdio() {
    let mut attributes = std::mem::MaybeUninit::<libc::termios>::uninit();
    assert_eq!(unsafe { libc::tcgetattr(0, attributes.as_mut_ptr()) }, 0);
    let mut attributes = unsafe { attributes.assume_init() };
    unsafe { libc::cfmakeraw(&mut attributes) };
    attributes.c_lflag |= libc::ISIG;
    assert_eq!(unsafe { libc::tcsetattr(0, libc::TCSANOW, &attributes) }, 0);
}
#[cfg(unix)]
fn child_size() {
    let mut size = std::mem::MaybeUninit::<libc::winsize>::uninit();
    assert_eq!(
        unsafe { libc::ioctl(0, libc::TIOCGWINSZ, size.as_mut_ptr()) },
        0
    );
    let size = unsafe { size.assume_init() };
    println!("SIZE:{},{}", size.ws_col, size.ws_row);
    println!("PIXELS:{},{}", size.ws_xpixel, size.ws_ypixel);
}
