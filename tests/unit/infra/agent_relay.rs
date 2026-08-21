use super::*;

/// deck ships one relay build per container architecture and streams the right
/// one in, so the committed artifacts have to be *the thing they claim to be*:
/// swapped files, or a truncated commit, would surface as an `exec` failure
/// inside somebody's container with nothing pointing back here.
#[test]
fn the_committed_artifacts_are_static_elfs_for_the_arch_they_name() {
    // ELF machine numbers, e_machine at offset 18 (little-endian, ELF64).
    for (arch, machine) in [("x86_64", 0x3e_u16), ("aarch64", 0xb7)] {
        let binary = relay_binary(arch).unwrap_or_else(|| panic!("no relay for {arch}"));
        assert!(
            binary.len() > 64,
            "{arch}: truncated: {} bytes",
            binary.len()
        );
        assert_eq!(&binary[..4], b"\x7fELF", "{arch}: not an ELF");
        assert_eq!(binary[4], 2, "{arch}: not 64-bit");
        assert_eq!(binary[5], 1, "{arch}: not little-endian");
        assert_eq!(
            u16::from_le_bytes([binary[18], binary[19]]),
            machine,
            "{arch}: built for the wrong machine"
        );
        // Statically linked: an executable with an interpreter would need a
        // dynamic loader the image might not have, which is the whole reason
        // these are musl static builds.
        assert!(
            !binary.windows(13).any(|w| w == b"/lib/ld-linux"),
            "{arch}: links a dynamic loader"
        );
    }

    // Both spellings each engine reports, and nothing invented for the rest.
    assert!(relay_binary("amd64").is_some());
    assert!(relay_binary("arm64").is_some());
    assert!(relay_binary("riscv64").is_none());
    assert!(relay_binary("").is_none());
}

/// The committed binary, run for real and driven over the protocol deck speaks
/// to it. Only possible where it can execute, which is the CI runner and any
/// Linux dev box; on macOS the artifact is a foreign ELF and the test is absent.
///
/// This is the check that the *artifact* — not the source it was built from —
/// still works: it accepts a connection, frames what it reads onto stdout under
/// a fresh channel id, delivers what deck frames back, and reports the close.
#[cfg(target_os = "linux")]
#[test]
fn the_committed_relay_binary_proxies_a_channel() {
    use std::io::{Read, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixStream;

    let arch = if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "x86_64"
    };
    let dir = std::env::temp_dir().join(format!("deck-relay-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let binary = dir.join("relay");
    std::fs::write(&binary, relay_binary(arch).expect("artifact")).expect("write relay");
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).expect("chmod");
    let socket = dir.join("agent.sock");

    let mut child = std::process::Command::new(&binary)
        .arg(&socket)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn the relay");

    // Readiness comes on stderr; stdout is the mux.
    let mut ready = String::new();
    std::io::BufRead::read_line(
        &mut std::io::BufReader::new(child.stderr.take().expect("stderr")),
        &mut ready,
    )
    .expect("read the readiness line");
    assert!(ready.contains(READY_MARKER), "relay said {ready:?}");

    let mut client = UnixStream::connect(&socket).expect("connect as a pane would");
    client.write_all(b"request").expect("send a request");

    let mut stdout = child.stdout.take().expect("stdout");
    let mut decoder = FrameDecoder::default();
    let mut read_frame = || loop {
        if let Some(frame) = decoder.next_frame().expect("valid framing") {
            return frame;
        }
        let mut chunk = [0u8; 512];
        let read = stdout.read(&mut chunk).expect("relay stdout");
        assert!(read > 0, "the relay closed its stdout");
        decoder.push(&chunk[..read]);
    };

    let (id, payload) = read_frame();
    assert_eq!(payload, b"request", "the relay reframed the request");
    assert_eq!(id, 1, "ids are minted by the accepting side, from 1");

    // Reply the way deck's mux does, and read it out of the socket.
    let mut stdin = child.stdin.take().expect("stdin");
    stdin
        .write_all(&encode_frame(id, b"answer"))
        .and_then(|()| stdin.flush())
        .expect("frame a reply");
    let mut answer = [0u8; 6];
    client.read_exact(&mut answer).expect("read the reply");
    assert_eq!(&answer, b"answer");

    // A pane hanging up is a close frame, not silence.
    drop(client);
    let (closed_id, closed) = read_frame();
    assert_eq!((closed_id, closed), (id, Vec::new()));

    // Closing deck's end is how the whole chain unwinds, and the relay must
    // clean up its socket on the way out rather than leaving a dead address.
    drop(stdin);
    let status = child.wait().expect("the relay exits when stdin closes");
    assert!(status.success(), "relay exited with {status}");
    assert!(!socket.exists(), "the relay left its socket behind");
    let _ = std::fs::remove_dir_all(&dir);
}
