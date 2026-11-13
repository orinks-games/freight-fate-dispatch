//! The game must never open a terminal window beside itself.
//!
//! That is one field in the executable's PE header, written by the linker
//! from `#![windows_subsystem = "windows"]` in main.rs. Nothing inside a
//! running process can see it, so the assertion reads the built binary.

#![cfg(windows)]

/// `IMAGE_SUBSYSTEM_WINDOWS_GUI`: Windows creates no console for the process.
const IMAGE_SUBSYSTEM_WINDOWS_GUI: u16 = 2;

/// The `Subsystem` field: `e_lfanew` (0x3C) points at `PE\0\0`, the COFF
/// header is the next 20 bytes, and `Subsystem` is 68 bytes into the optional
/// header in both PE32 and PE32+.
fn pe_subsystem(bytes: &[u8]) -> u16 {
    let u16_at = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
    assert_eq!(&bytes[..2], b"MZ", "not a PE image");
    let pe = u32::from_le_bytes(bytes[0x3C..0x40].try_into().unwrap()) as usize;
    assert_eq!(&bytes[pe..pe + 4], b"PE\0\0", "no PE signature");
    u16_at(pe + 4 + 20 + 68)
}

#[test]
fn game_binary_opens_no_terminal_window() {
    let exe = env!("CARGO_BIN_EXE_FreightFateDispatch");
    let bytes = std::fs::read(exe).unwrap_or_else(|e| panic!("cannot read {exe}: {e}"));
    assert_eq!(
        pe_subsystem(&bytes),
        IMAGE_SUBSYSTEM_WINDOWS_GUI,
        "{exe} is not a GUI-subsystem image, so launching it opens a terminal \
         window beside the game. Restore `#![cfg_attr(windows, windows_subsystem \
         = \"windows\")]` at the top of src/main.rs."
    );
}
