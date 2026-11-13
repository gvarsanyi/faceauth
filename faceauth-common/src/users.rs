//! Thread-safe passwd lookups (reentrant `_r` libc variants only).

use std::ffi::{CStr, CString};

/// A resolved passwd entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub name: String,
    pub uid: u32,
}

/// Call a reentrant getpw*_r function, growing the buffer on ERANGE.
fn getpw_r(
    lookup: impl Fn(*mut libc::passwd, *mut libc::c_char, usize, *mut *mut libc::passwd) -> libc::c_int,
) -> Option<User> {
    let mut size = 1024usize;
    loop {
        let mut buf = vec![0u8; size];
        let mut pwd = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut result: *mut libc::passwd = std::ptr::null_mut();
        let ret = lookup(pwd.as_mut_ptr(), buf.as_mut_ptr().cast(), buf.len(), &mut result);
        if ret == libc::ERANGE && size < 1 << 20 {
            size *= 4;
            continue;
        }
        if ret != 0 || result.is_null() {
            return None;
        }
        // SAFETY: on success `result` points to `pwd`, whose string fields
        // point into `buf`; both are alive for the rest of this scope.
        let pw = unsafe { &*result };
        let name = unsafe { CStr::from_ptr(pw.pw_name) }.to_str().ok()?.to_string();
        return Some(User { name, uid: pw.pw_uid });
    }
}

/// Look up a user by name.
pub fn by_name(name: &str) -> Option<User> {
    let cname = CString::new(name).ok()?;
    // SAFETY: all pointers are valid for the duration of the call.
    getpw_r(|pwd, buf, len, res| unsafe { libc::getpwnam_r(cname.as_ptr(), pwd, buf, len, res) })
}

/// Look up a user by UID.
pub fn by_uid(uid: u32) -> Option<User> {
    // SAFETY: all pointers are valid for the duration of the call.
    getpw_r(|pwd, buf, len, res| unsafe { libc::getpwuid_r(uid, pwd, buf, len, res) })
}

/// Real UID of the current process.
pub fn current_uid() -> u32 {
    // SAFETY: getuid cannot fail.
    unsafe { libc::getuid() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_resolves_both_ways() {
        let root = by_uid(0).expect("uid 0 must exist");
        assert_eq!(root.uid, 0);
        assert_eq!(by_name(&root.name).map(|u| u.uid), Some(0));
    }

    #[test]
    fn unknown_user_is_none() {
        assert!(by_name("no-such-user-faceauth-test").is_none());
        assert!(by_name("bad\0name").is_none());
    }

}
