use thiserror::Error;

mod emergency;
pub use emergency::{EmergencyJit, JitCompilation, JitError, JitEvent};

pub const CRATE_NAME: &str = "nx86-jit";

#[must_use]
pub const fn crate_name() -> &'static str {
    CRATE_NAME
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatchStage {
    BoundsCheck,
    MakeWritable,
    RestoreExecutable,
}

impl std::fmt::Display for PatchStage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BoundsCheck => formatter.write_str("bounds check"),
            Self::MakeWritable => formatter.write_str("make writable"),
            Self::RestoreExecutable => formatter.write_str("restore executable permissions"),
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ExecError {
    #[error("cannot allocate executable memory for empty code")]
    EmptyCode,
    #[error("native execution is unavailable on {os}/{arch}")]
    UnsupportedHost {
        os: &'static str,
        arch: &'static str,
    },
    #[error("executable memory allocation failed: {message}")]
    Allocation { message: String },
    #[error("executable memory permission change failed: {message}")]
    Permission { message: String },
    #[error("executable memory patch failed during {stage}: {message}")]
    Patch { stage: PatchStage, message: String },
    #[error("executable memory release failed: {message}")]
    Release { message: String },
}

#[derive(Debug)]
pub struct ExecutableMemory {
    inner: platform::ExecutableMemory,
}

impl ExecutableMemory {
    pub fn new(code: &[u8]) -> Result<Self, ExecError> {
        if code.is_empty() {
            return Err(ExecError::EmptyCode);
        }
        Ok(Self {
            inner: platform::ExecutableMemory::new(code)?,
        })
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.inner.len()
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Address of the first emitted byte in this executable mapping.
    pub fn entry_addr(&self) -> Result<usize, ExecError> {
        self.inner.entry_addr()
    }

    /// Rewrite a byte range in this executable mapping while preserving W^X.
    ///
    /// # Safety
    ///
    /// The caller must ensure `bytes` form valid machine code at `offset` and
    /// that no thread can execute this mapping while it is being patched.
    #[allow(unsafe_code)]
    pub unsafe fn patch(&self, offset: usize, bytes: &[u8]) -> Result<(), ExecError> {
        // SAFETY: The caller upholds the generated-code and exclusive-access
        // contract; the platform implementation validates the byte range.
        unsafe { self.inner.patch(offset, bytes) }
    }

    /// Call this executable mapping as `extern "C" fn() -> u64`.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the mapped bytes contain trusted machine
    /// code with this exact ABI and that executing it cannot violate Rust's
    /// memory-safety rules.
    #[allow(unsafe_code)]
    pub unsafe fn call_u64(&self) -> Result<u64, ExecError> {
        // SAFETY: The caller upholds the generated-code ABI contract.
        unsafe { self.inner.call_u64() }
    }

    /// Call this executable mapping as `extern "C" fn(*mut T)`.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the mapped bytes contain trusted machine
    /// code with this exact ABI and that the code treats `state` as a valid,
    /// exclusive mutable pointer for the duration of the call.
    #[allow(unsafe_code)]
    pub unsafe fn call_with_state<T>(&self, state: &mut T) -> Result<(), ExecError> {
        // SAFETY: The caller upholds the generated-code ABI and state-pointer
        // contract.
        unsafe { self.inner.call_with_state(state) }
    }
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[allow(unsafe_code)]
mod platform {
    use std::{cell::Cell, ffi::c_void, ptr};

    use super::{ExecError, PatchStage};

    #[derive(Debug)]
    pub struct ExecutableMemory {
        ptr: *mut c_void,
        len: usize,
        poisoned: Cell<bool>,
    }

    impl ExecutableMemory {
        pub fn new(code: &[u8]) -> Result<Self, ExecError> {
            let len = code.len();
            // SAFETY: This creates an anonymous private mapping owned by this
            // object. The mapping is writable only for the copy below, then is
            // switched to read+execute before any function pointer is created.
            let ptr = unsafe {
                libc::mmap(
                    ptr::null_mut(),
                    len,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                    -1,
                    0,
                )
            };
            if ptr == libc::MAP_FAILED {
                return Err(ExecError::Allocation {
                    message: std::io::Error::last_os_error().to_string(),
                });
            }

            // SAFETY: `ptr` points at a writable mapping of at least `len`
            // bytes, and `code` is a valid non-overlapping source slice.
            unsafe {
                ptr::copy_nonoverlapping(code.as_ptr(), ptr.cast::<u8>(), len);
            }

            // SAFETY: `ptr` and `len` describe the mapping just returned by
            // mmap. After this succeeds the mapping is no longer writable.
            let protect_result =
                unsafe { libc::mprotect(ptr, len, libc::PROT_READ | libc::PROT_EXEC) };
            if protect_result != 0 {
                let message = std::io::Error::last_os_error().to_string();
                // SAFETY: Best-effort cleanup of the mapping created above.
                let _ = unsafe { libc::munmap(ptr, len) };
                return Err(ExecError::Permission { message });
            }

            Ok(Self {
                ptr,
                len,
                poisoned: Cell::new(false),
            })
        }

        #[must_use]
        pub const fn len(&self) -> usize {
            self.len
        }

        pub fn entry_addr(&self) -> Result<usize, ExecError> {
            if self.poisoned.get() {
                return Err(poisoned_mapping_error());
            }
            Ok(self.ptr as usize)
        }

        pub unsafe fn patch(&self, offset: usize, bytes: &[u8]) -> Result<(), ExecError> {
            if self.poisoned.get() {
                return Err(poisoned_mapping_error());
            }
            let Some(end) = offset.checked_add(bytes.len()) else {
                return Err(ExecError::Patch {
                    stage: PatchStage::BoundsCheck,
                    message: "patch range overflowed usize".to_owned(),
                });
            };
            if end > self.len {
                return Err(ExecError::Patch {
                    stage: PatchStage::BoundsCheck,
                    message: format!(
                        "range {offset}..{end} exceeds executable length {}",
                        self.len
                    ),
                });
            }
            if bytes.is_empty() {
                return Ok(());
            }

            // SAFETY: `ptr` and `len` describe the complete live mapping. It is
            // made writable without execute permission, preserving W^X.
            let writable_result =
                unsafe { libc::mprotect(self.ptr, self.len, libc::PROT_READ | libc::PROT_WRITE) };
            if writable_result != 0 {
                return Err(ExecError::Patch {
                    stage: PatchStage::MakeWritable,
                    message: std::io::Error::last_os_error().to_string(),
                });
            }

            // SAFETY: The validated patch range lies inside the writable mapping,
            // and the caller guarantees no concurrent execution or overlapping
            // mutation.
            unsafe {
                ptr::copy_nonoverlapping(
                    bytes.as_ptr(),
                    self.ptr.cast::<u8>().add(offset),
                    bytes.len(),
                );
            }

            // SAFETY: Restore read+execute permission across the full mapping
            // before it can be called again.
            let executable_result =
                unsafe { libc::mprotect(self.ptr, self.len, libc::PROT_READ | libc::PROT_EXEC) };
            if executable_result != 0 {
                self.poisoned.set(true);
                return Err(ExecError::Patch {
                    stage: PatchStage::RestoreExecutable,
                    message: std::io::Error::last_os_error().to_string(),
                });
            }
            Ok(())
        }

        pub unsafe fn call_u64(&self) -> Result<u64, ExecError> {
            if self.poisoned.get() {
                return Err(poisoned_mapping_error());
            }
            // SAFETY: The bytes in this mapping are trusted internal generated
            // code and this wrapper is used only for functions with this ABI.
            let function: extern "C" fn() -> u64 = unsafe { std::mem::transmute(self.ptr) };
            Ok(function())
        }

        pub unsafe fn call_with_state<T>(&self, state: &mut T) -> Result<(), ExecError> {
            if self.poisoned.get() {
                return Err(poisoned_mapping_error());
            }
            // SAFETY: The bytes in this mapping are trusted internal generated
            // code and this wrapper is used only for functions taking exactly a
            // mutable state pointer as their first argument.
            let function: extern "C" fn(*mut T) = unsafe { std::mem::transmute(self.ptr) };
            function(state as *mut T);
            Ok(())
        }
    }

    fn poisoned_mapping_error() -> ExecError {
        ExecError::Patch {
            stage: PatchStage::RestoreExecutable,
            message: "mapping is quarantined after an RX-restore failure".to_owned(),
        }
    }

    impl Drop for ExecutableMemory {
        fn drop(&mut self) {
            // SAFETY: `ptr` and `len` are the live mapping created by `new`.
            let _ = unsafe { libc::munmap(self.ptr, self.len) };
        }
    }
}

#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
#[allow(unsafe_code)]
mod platform {
    use super::ExecError;

    #[derive(Debug)]
    pub struct ExecutableMemory;

    impl ExecutableMemory {
        pub const fn new(_code: &[u8]) -> Result<Self, ExecError> {
            Err(ExecError::UnsupportedHost {
                os: std::env::consts::OS,
                arch: std::env::consts::ARCH,
            })
        }

        #[must_use]
        pub const fn len(&self) -> usize {
            0
        }

        pub const fn entry_addr(&self) -> Result<usize, ExecError> {
            Err(ExecError::UnsupportedHost {
                os: std::env::consts::OS,
                arch: std::env::consts::ARCH,
            })
        }

        pub const unsafe fn patch(&self, _offset: usize, _bytes: &[u8]) -> Result<(), ExecError> {
            Err(ExecError::UnsupportedHost {
                os: std::env::consts::OS,
                arch: std::env::consts::ARCH,
            })
        }

        pub const unsafe fn call_u64(&self) -> Result<u64, ExecError> {
            Err(ExecError::UnsupportedHost {
                os: std::env::consts::OS,
                arch: std::env::consts::ARCH,
            })
        }

        pub const unsafe fn call_with_state<T>(&self, _state: &mut T) -> Result<(), ExecError> {
            Err(ExecError::UnsupportedHost {
                os: std::env::consts::OS,
                arch: std::env::consts::ARCH,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ExecError, ExecutableMemory};

    #[test]
    fn rejects_empty_code() {
        assert_eq!(
            ExecutableMemory::new(&[]).expect_err("empty code"),
            ExecError::EmptyCode
        );
    }

    #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
    #[test]
    fn reports_unsupported_host() {
        assert!(matches!(
            ExecutableMemory::new(&[0xC3]),
            Err(ExecError::UnsupportedHost { .. })
        ));
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    #[test]
    #[allow(unsafe_code)]
    fn calls_generated_u64_function() {
        let mut asm = nx86_x64_asm::Assembler::new();
        asm.mov_reg_imm64(nx86_x64_asm::Reg64::Rax, 42);
        asm.ret();
        let code = asm.finish().expect("assembler should finish");

        let executable = ExecutableMemory::new(code.bytes()).expect("code should allocate");

        // SAFETY: The test emits `mov rax, imm64; ret`, which has the expected
        // `extern "C" fn() -> u64` ABI and touches no memory.
        let result = unsafe { executable.call_u64() }.expect("function should run");
        assert_eq!(result, 42);
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    #[test]
    #[allow(unsafe_code)]
    fn patches_and_restores_generated_code() {
        let mut asm = nx86_x64_asm::Assembler::new();
        asm.mov_reg_imm64(nx86_x64_asm::Reg64::Rax, 1);
        asm.ret();
        let code = asm.finish().expect("assembler should finish");
        let executable = ExecutableMemory::new(code.bytes()).expect("code should allocate");
        assert!(executable.entry_addr().is_ok());

        let replacement = 2_u64.to_le_bytes();
        // SAFETY: The immediate starts two bytes into `mov rax, imm64`; the
        // replacement preserves the instruction shape and nothing executes the
        // mapping concurrently in this test.
        unsafe { executable.patch(2, &replacement) }.expect("patch immediate");
        let maps = std::fs::read_to_string("/proc/self/maps").expect("read process maps");
        let entry = executable.entry_addr().expect("entry address");
        let permissions = maps
            .lines()
            .find_map(|line| {
                let mut fields = line.split_whitespace();
                let range = fields.next()?;
                let permissions = fields.next()?;
                let (start, end) = range.split_once('-')?;
                let start = usize::from_str_radix(start, 16).ok()?;
                let end = usize::from_str_radix(end, 16).ok()?;
                (start <= entry && entry < end).then_some(permissions)
            })
            .expect("find executable mapping");
        assert!(permissions.contains('x'));
        assert!(!permissions.contains('w'));
        // SAFETY: The mapping still contains `mov rax, imm64; ret`.
        assert_eq!(
            unsafe { executable.call_u64() }.expect("call patched code"),
            2
        );

        let original = 1_u64.to_le_bytes();
        // SAFETY: Same valid immediate range and exclusive access as above.
        unsafe { executable.patch(2, &original) }.expect("restore immediate");
        // SAFETY: The mapping still contains `mov rax, imm64; ret`.
        assert_eq!(
            unsafe { executable.call_u64() }.expect("call restored code"),
            1
        );
    }
}
