//! Port of `ext/random/csprng.c`: the operating system's CSPRNG behind
//! `random_bytes()` and `random_int()`.
//!
//! The source per platform is the reference's:
//!
//! | platform | source |
//! |---|---|
//! | macOS | `CCRandomGenerateBytes` (CommonCrypto) |
//! | OpenBSD, NetBSD, Haiku | `arc4random_buf` |
//! | Linux | `getrandom(2)` syscall, then `/dev/urandom` |
//! | FreeBSD, DragonFly, Solaris, illumos | `getrandom(3)`, then `/dev/urandom` |
//! | other Unix | `/dev/urandom` |
//!
//! A failure is reported with the reference's `errstr` text, thrown as
//! `Random\RandomException` by [`random_bytes_throw`].

use crate::stdlib::common::throws;
use crate::stdlib::fileres::strerror;

/// `php_random_bytes_ex`: fill `bytes` from the OS CSPRNG, or return the
/// reference's error text.
#[cfg(target_os = "macos")]
pub(crate) fn random_bytes_ex(bytes: &mut [u8]) -> Result<(), String> {
    // CommonCrypto/CommonRandom.h; `kCCSuccess` is 0. The symbol lives in
    // libSystem, which every macOS binary links.
    extern "C" {
        fn CCRandomGenerateBytes(bytes: *mut libc::c_void, count: libc::size_t) -> i32;
    }
    // SAFETY: the pointer and length describe `bytes`, which is writable.
    if unsafe { CCRandomGenerateBytes(bytes.as_mut_ptr().cast(), bytes.len()) } != 0 {
        return Err(
            "Failed to retrieve randomness from the operating system (CCRandomGenerateBytes)"
                .to_string(),
        );
    }
    Ok(())
}

/// `php_random_bytes_ex` on the platforms whose `arc4random_buf` the reference
/// uses because it cannot fail.
#[cfg(any(target_os = "openbsd", target_os = "netbsd", target_os = "haiku"))]
pub(crate) fn random_bytes_ex(bytes: &mut [u8]) -> Result<(), String> {
    // SAFETY: the pointer and length describe `bytes`, which is writable.
    unsafe { libc::arc4random_buf(bytes.as_mut_ptr().cast(), bytes.len()) };
    Ok(())
}

/// `php_random_bytes_ex` everywhere else: `getrandom` where the platform has
/// it, falling back to `/dev/urandom` when it is missing (`ENOSYS`) or fails.
#[cfg(not(any(
    target_os = "macos",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "haiku"
)))]
pub(crate) fn random_bytes_ex(bytes: &mut [u8]) -> Result<(), String> {
    if getrandom_fill(bytes) < bytes.len() {
        urandom_fill(urandom_fd()?, bytes)?;
    }
    Ok(())
}

/// The `getrandom` loop: how many bytes it filled before finishing or giving up.
/// `EINTR`/`EAGAIN` retry; any other error stops it so the caller falls back.
#[cfg(any(
    target_os = "linux",
    target_os = "android",
    target_os = "freebsd",
    target_os = "dragonfly",
    target_os = "solaris",
    target_os = "illumos"
))]
fn getrandom_fill(bytes: &mut [u8]) -> usize {
    let mut read = 0;
    while read < bytes.len() {
        let rest = &mut bytes[read..];
        // SAFETY: the pointer and length describe `rest`, which is writable.
        #[cfg(any(target_os = "linux", target_os = "android"))]
        let n = unsafe {
            libc::syscall(libc::SYS_getrandom, rest.as_mut_ptr(), rest.len(), 0) as isize
        };
        // SAFETY: as above.
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        let n = unsafe { libc::getrandom(rest.as_mut_ptr().cast(), rest.len(), 0) };
        if n == -1 {
            match std::io::Error::last_os_error().raw_os_error() {
                Some(libc::EINTR) | Some(libc::EAGAIN) => continue,
                _ => break,
            }
        }
        read += n as usize;
    }
    read
}

/// No `getrandom` on this platform: everything comes from `/dev/urandom`.
#[cfg(not(any(
    target_os = "macos",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "haiku",
    target_os = "linux",
    target_os = "android",
    target_os = "freebsd",
    target_os = "dragonfly",
    target_os = "solaris",
    target_os = "illumos"
)))]
fn getrandom_fill(_bytes: &mut [u8]) -> usize {
    0
}

/// The process-wide `/dev/urandom` descriptor, opened once and shared as the
/// reference's `random_fd` is: a thread that loses the race to publish its
/// descriptor closes it and uses the winner's.
#[cfg_attr(
    any(
        target_os = "macos",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "haiku"
    ),
    allow(dead_code)
)]
fn urandom_fd() -> Result<libc::c_int, String> {
    use std::sync::atomic::{AtomicI32, Ordering};
    static RANDOM_FD: AtomicI32 = AtomicI32::new(-1);
    let fd = RANDOM_FD.load(Ordering::Acquire);
    if fd >= 0 {
        return Ok(fd);
    }
    let fd = open_urandom(c"/dev/urandom")?;
    match RANDOM_FD.compare_exchange(-1, fd, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => Ok(fd),
        Err(winner) => {
            // SAFETY: `fd` was opened above and never published.
            unsafe { libc::close(fd) };
            Ok(winner)
        }
    }
}

/// Open `path` read-only and require a character device, with the reference's
/// error text — which names `/dev/urandom` whatever `path` is, since the
/// reference only ever opens that one.
#[cfg_attr(
    any(
        target_os = "macos",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "haiku"
    ),
    allow(dead_code)
)]
fn open_urandom(path: &std::ffi::CStr) -> Result<libc::c_int, String> {
    // SAFETY: `path` is NUL-terminated.
    let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDONLY) };
    if fd < 0 {
        return Err(with_errno("Cannot open /dev/urandom"));
    }
    // SAFETY: an all-zero `stat` is a valid out-buffer for `fstat`.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // The reference clears `errno` before `fstat` and appends `strerror(errno)`
    // only when it is set: a failed `fstat` carries one, while a file that is
    // merely not a character device reports the bare message.
    // SAFETY: `fd` is open and `st` is writable.
    let failed = unsafe { libc::fstat(fd, &mut st) } != 0;
    if failed || (st.st_mode & libc::S_IFMT) != libc::S_IFCHR {
        let msg = if failed {
            with_errno("Error reading from /dev/urandom")
        } else {
            "Error reading from /dev/urandom".to_string()
        };
        // SAFETY: `fd` is open and owned here.
        unsafe { libc::close(fd) };
        return Err(msg);
    }
    Ok(fd)
}

/// Fill `bytes` by `read(2)` from `fd`; a read that returns 0 or fails is
/// "Could not gather sufficient random data".
#[cfg_attr(
    any(
        target_os = "macos",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "haiku"
    ),
    allow(dead_code)
)]
fn urandom_fill(fd: libc::c_int, bytes: &mut [u8]) -> Result<(), String> {
    let mut read = 0;
    while read < bytes.len() {
        let rest = &mut bytes[read..];
        // SAFETY: the pointer and length describe `rest`, which is writable.
        let n = unsafe { libc::read(fd, rest.as_mut_ptr().cast(), rest.len()) };
        // End of file leaves `errno` at the 0 the reference cleared it to, so
        // only a failed read carries `strerror`.
        if n == 0 {
            return Err("Could not gather sufficient random data".to_string());
        }
        if n < 0 {
            return Err(with_errno("Could not gather sufficient random data"));
        }
        read += n as usize;
    }
    Ok(())
}

/// `msg: strerror(errno)` for a call that has just failed and set `errno`.
#[cfg_attr(
    any(
        target_os = "macos",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "haiku"
    ),
    allow(dead_code)
)]
fn with_errno(msg: &str) -> String {
    format!("{msg}: {}", strerror(&std::io::Error::last_os_error()))
}

/// `php_random_bytes_throw`: [`random_bytes_ex`] with a failure thrown as
/// `Random\RandomException`.
pub(crate) fn random_bytes_throw(bytes: &mut [u8]) -> Result<(), String> {
    random_bytes_ex(bytes).map_err(|e| throws("Random\\RandomException", e))
}

/// `php_random_int_throw`: a uniform integer in `[min, max]` (`min <= max`,
/// which the caller has checked) from the OS CSPRNG.
pub(crate) fn random_int_throw(min: i64, max: i64) -> Result<i64, String> {
    range(min, max, || {
        let mut trial = [0u8; 8];
        random_bytes_throw(&mut trial)?;
        Ok(u64::from_ne_bytes(trial))
    })
}

/// `php_random_int`'s mapping of 64-bit draws onto `[min, max]`. A span that is
/// a power of two masks cleanly; any other rejects draws above the largest
/// multiple of the span, so no residue is favoured.
fn range(min: i64, max: i64, mut draw: impl FnMut() -> Result<u64, String>) -> Result<i64, String> {
    if min == max {
        return Ok(min);
    }
    let umax = (max as u64).wrapping_sub(min as u64);
    let mut trial = draw()?;
    if umax == u64::MAX {
        return Ok(trial as i64);
    }
    let umax = umax + 1;
    if umax & (umax - 1) != 0 {
        let limit = u64::MAX - (u64::MAX % umax) - 1;
        while trial > limit {
            trial = draw()?;
        }
    }
    Ok((trial % umax).wrapping_add(min as u64) as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feeds `range` a fixed sequence of draws and records how many it took.
    fn range_over(min: i64, max: i64, draws: &[u64]) -> (i64, usize) {
        let mut it = draws.iter();
        let mut taken = 0;
        let v = range(min, max, || {
            taken += 1;
            Ok(*it.next().expect("range drew more than supplied"))
        })
        .unwrap();
        (v, taken)
    }

    #[test]
    fn equal_bounds_draw_nothing() {
        assert_eq!(range_over(7, 7, &[]), (7, 0));
    }

    #[test]
    fn full_width_span_is_the_raw_draw() {
        assert_eq!(range_over(i64::MIN, i64::MAX, &[u64::MAX]), (-1, 1));
    }

    #[test]
    fn power_of_two_span_never_rejects() {
        // span 4: u64::MAX % 4 == 3, so the top draw is kept.
        assert_eq!(range_over(10, 13, &[u64::MAX]), (13, 1));
    }

    #[test]
    fn draws_above_the_limit_are_rejected() {
        // span 3: limit = u64::MAX - (u64::MAX % 3) - 1 = u64::MAX - 1, so
        // u64::MAX is redrawn and the next draw (5 % 3 == 2) is used.
        assert_eq!(range_over(0, 2, &[u64::MAX, 5]), (2, 2));
        assert_eq!(
            range_over(0, 2, &[u64::MAX - 1]),
            (((u64::MAX - 1) % 3) as i64, 1)
        );
    }

    #[test]
    fn negative_bounds_offset_without_overflow() {
        assert_eq!(range_over(-5, -3, &[4]), (-4, 1));
        assert_eq!(range_over(i64::MIN, i64::MIN + 1, &[1]), (i64::MIN + 1, 1));
    }

    #[test]
    fn urandom_errors_use_the_reference_text() {
        assert_eq!(
            open_urandom(c"/nonexistent/phplang-urandom").unwrap_err(),
            "Cannot open /dev/urandom: No such file or directory"
        );
        // A regular file opens and `fstat`s fine but is not a character device.
        assert_eq!(
            open_urandom(c"/etc/hosts").unwrap_err(),
            "Error reading from /dev/urandom"
        );
    }

    #[test]
    fn urandom_short_read_is_insufficient_data() {
        // SAFETY: `/dev/null` is NUL-terminated; it reads EOF at once.
        let fd = unsafe { libc::open(c"/dev/null".as_ptr(), libc::O_RDONLY) };
        assert!(fd >= 0);
        let mut buf = [0u8; 4];
        assert_eq!(
            urandom_fill(fd, &mut buf).unwrap_err(),
            "Could not gather sufficient random data"
        );
        // SAFETY: `fd` was opened above.
        unsafe { libc::close(fd) };
    }

    #[test]
    fn os_source_fills_the_buffer() {
        // Not a randomness test: 64 zero bytes from a working source has
        // probability 2^-512, so an all-zero buffer means nothing was written.
        let mut buf = [0u8; 64];
        random_bytes_ex(&mut buf).unwrap();
        assert!(buf.iter().any(|&b| b != 0));
    }
}
