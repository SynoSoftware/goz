//! The SHA-256 of the gozlerd.exe that the setup's build paired with this
//! window. `installer\Pair.ps1` writes it into the built gozler-ui.exe after
//! [`MARKER`], because cargo cannot make the window's link wait for
//! gozlerd.exe (DESIGN.md: Service fixes).

/// Finds the hash in the file; `installer\Pair.ps1` holds the same text, and
/// the file must hold it once.
const MARKER: [u8; 32] = *b"gozler-ui: paired gozlerd sha256";

#[repr(C)]
struct Pairing {
    marker: [u8; 32],
    /// All zeros until `installer\Pair.ps1` writes the hash.
    sha256: [u8; 32],
}

static PAIRING: Pairing = Pairing {
    marker: MARKER,
    sha256: [0; 32],
};

/// The SHA-256 of the paired gozlerd.exe; `None` in a window that was never
/// paired, such as one from a plain `cargo build`.
pub(crate) fn paired_gozlerd() -> Option<[u8; 32]> {
    // SAFETY: the static is aligned, initialized and never written in
    // memory. The read is volatile so the compiler reads the bytes the file
    // holds instead of the zeros it compiled.
    let sha256 = unsafe { std::ptr::read_volatile(&PAIRING.sha256) };
    (sha256 != [0; 32]).then_some(sha256)
}
