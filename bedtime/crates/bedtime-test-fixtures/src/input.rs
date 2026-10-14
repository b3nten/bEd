pub fn child() {
    use std::io::{Read, Write};
    // SAFETY: stdin is this child's live PTY; libc initializes the termios value
    // before it is read and cfmakeraw only mutates that initialized value.
    unsafe {
        let mut attributes = std::mem::MaybeUninit::<libc::termios>::uninit();
        assert_eq!(
            libc::tcgetattr(libc::STDIN_FILENO, attributes.as_mut_ptr()),
            0
        );
        let mut attributes = attributes.assume_init();
        libc::cfmakeraw(&mut attributes);
        assert_eq!(
            libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &attributes),
            0
        );
    }
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(b"\x1b[>1u\x1b[?2004hINPUT_READY\r\n")
        .unwrap();
    stdout.flush().unwrap();
    let expected = "\x1b[13;2u\x1b[99;5u\x1b[118;5u\x1b[200~one\rtwo\r界\x1b[201~日本語";
    let mut received = vec![0; expected.len()];
    std::io::stdin().read_exact(&mut received).unwrap();
    assert_eq!(received, expected.as_bytes());
    stdout.write_all(b"\x1b[<u\x1b[?2004lINPUT_OK\r\n").unwrap();
    stdout.flush().unwrap();
}
