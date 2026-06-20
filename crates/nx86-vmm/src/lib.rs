use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

const GIB: u64 = 1024 * 1024 * 1024;
pub const ARENA_SIZE_BYTES: u64 = 64 * GIB;
pub const PAGE_SIZE: u64 = 4096;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(transparent)]
pub struct GuestAddress(pub u64);

impl GuestAddress {
    #[must_use]
    pub const fn page_base(self) -> u64 {
        self.0 & !(PAGE_SIZE - 1)
    }

    #[must_use]
    pub const fn page_offset(self) -> usize {
        (self.0 & (PAGE_SIZE - 1)) as usize
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct PagePermissions {
    pub read: bool,
    pub write: bool,
    pub execute: bool,
}

impl PagePermissions {
    pub const READ: Self = Self {
        read: true,
        write: false,
        execute: false,
    };
    pub const READ_WRITE: Self = Self {
        read: true,
        write: true,
        execute: false,
    };
    pub const READ_EXECUTE: Self = Self {
        read: true,
        write: false,
        execute: true,
    };
}

#[derive(Debug)]
pub struct GuestMemory {
    _arena: arena::ArenaReservation,
    pages: BTreeMap<u64, Page>,
}

impl GuestMemory {
    pub fn new() -> Result<Self, VmmFault> {
        Ok(Self {
            _arena: arena::ArenaReservation::new()?,
            pages: BTreeMap::new(),
        })
    }

    pub fn new_logical() -> Self {
        Self {
            _arena: arena::ArenaReservation::logical(),
            pages: BTreeMap::new(),
        }
    }

    #[must_use]
    pub const fn arena_size_bytes(&self) -> u64 {
        ARENA_SIZE_BYTES
    }

    pub fn map_page(
        &mut self,
        address: GuestAddress,
        permissions: PagePermissions,
    ) -> Result<(), VmmFault> {
        let page_base = GuestAddress(address.page_base());
        self.check_range(page_base, PAGE_SIZE as usize)?;
        self.pages.insert(
            page_base.0,
            Page {
                permissions,
                data: vec![0; PAGE_SIZE as usize],
            },
        );
        Ok(())
    }

    pub fn unmap_page(&mut self, address: GuestAddress) -> Result<(), VmmFault> {
        self.check_range(GuestAddress(address.page_base()), 1)?;
        self.pages.remove(&address.page_base());
        Ok(())
    }

    pub fn read(&self, address: GuestAddress, len: usize) -> Result<Vec<u8>, VmmFault> {
        self.check_range(address, len)?;
        self.validate_access(address, len, MemoryAccess::Read)?;
        let mut output = vec![0; len];
        self.copy_out(address, &mut output)?;
        Ok(output)
    }

    pub fn write(&mut self, address: GuestAddress, bytes: &[u8]) -> Result<(), VmmFault> {
        self.check_range(address, bytes.len())?;
        self.validate_access(address, bytes.len(), MemoryAccess::Write)?;
        let mut remaining = bytes;
        let mut current = address.0;

        while !remaining.is_empty() {
            let current_address = GuestAddress(current);
            let page_base = current_address.page_base();
            let offset = current_address.page_offset();
            let chunk_len = remaining.len().min(PAGE_SIZE as usize - offset);
            let page = self.pages.get_mut(&page_base).ok_or(VmmFault::Unmapped {
                address: GuestAddress(page_base),
            })?;
            if !page.permissions.write {
                return Err(VmmFault::Permission {
                    address: current_address,
                    access: MemoryAccess::Write,
                    permissions: page.permissions,
                });
            }

            page.data[offset..offset + chunk_len].copy_from_slice(&remaining[..chunk_len]);
            remaining = &remaining[chunk_len..];
            current += chunk_len as u64;
        }

        Ok(())
    }

    pub fn debug_dump(&self, address: GuestAddress, len: usize) -> Result<MemoryDump, VmmFault> {
        Ok(MemoryDump {
            start: address,
            bytes: self.read(address, len)?,
        })
    }

    fn copy_out(&self, address: GuestAddress, output: &mut [u8]) -> Result<(), VmmFault> {
        let mut written = 0;
        let mut current = address.0;

        while written < output.len() {
            let current_address = GuestAddress(current);
            let page_base = current_address.page_base();
            let offset = current_address.page_offset();
            let chunk_len = (output.len() - written).min(PAGE_SIZE as usize - offset);
            let page = self.pages.get(&page_base).ok_or(VmmFault::Unmapped {
                address: GuestAddress(page_base),
            })?;
            if !page.permissions.read {
                return Err(VmmFault::Permission {
                    address: current_address,
                    access: MemoryAccess::Read,
                    permissions: page.permissions,
                });
            }

            output[written..written + chunk_len]
                .copy_from_slice(&page.data[offset..offset + chunk_len]);
            written += chunk_len;
            current += chunk_len as u64;
        }

        Ok(())
    }

    fn validate_access(
        &self,
        address: GuestAddress,
        len: usize,
        access: MemoryAccess,
    ) -> Result<(), VmmFault> {
        let mut checked = 0;
        let mut current = address.0;

        while checked < len {
            let current_address = GuestAddress(current);
            let page_base = current_address.page_base();
            let offset = current_address.page_offset();
            let chunk_len = (len - checked).min(PAGE_SIZE as usize - offset);
            let page = self.pages.get(&page_base).ok_or(VmmFault::Unmapped {
                address: GuestAddress(page_base),
            })?;
            let allowed = match access {
                MemoryAccess::Read => page.permissions.read,
                MemoryAccess::Write => page.permissions.write,
                MemoryAccess::Execute => page.permissions.execute,
            };

            if !allowed {
                return Err(VmmFault::Permission {
                    address: current_address,
                    access,
                    permissions: page.permissions,
                });
            }

            checked += chunk_len;
            current += chunk_len as u64;
        }

        Ok(())
    }

    fn check_range(&self, address: GuestAddress, len: usize) -> Result<(), VmmFault> {
        let len_u64 = u64::try_from(len).map_err(|_| VmmFault::OutOfRange { address, len })?;
        let end = address
            .0
            .checked_add(len_u64)
            .ok_or(VmmFault::OutOfRange { address, len })?;
        if address.0 >= ARENA_SIZE_BYTES || end > ARENA_SIZE_BYTES {
            Err(VmmFault::OutOfRange { address, len })
        } else {
            Ok(())
        }
    }
}

#[derive(Debug)]
struct Page {
    permissions: PagePermissions,
    data: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct MemoryDump {
    pub start: GuestAddress,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum MemoryAccess {
    Read,
    Write,
    Execute,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum VmmFault {
    #[error("failed to reserve 64 GiB guest arena: {message}")]
    ArenaReservation { message: String },
    #[error("guest memory range at {address:?} with length {len} is outside the 64 GiB arena")]
    OutOfRange { address: GuestAddress, len: usize },
    #[error("guest page at {address:?} is not mapped")]
    Unmapped { address: GuestAddress },
    #[error("{access:?} access at {address:?} violates permissions {permissions:?}")]
    Permission {
        address: GuestAddress,
        access: MemoryAccess,
        permissions: PagePermissions,
    },
}

#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
mod arena {
    use super::{ARENA_SIZE_BYTES, VmmFault};

    #[derive(Debug)]
    pub struct ArenaReservation {
        base: usize,
    }

    impl ArenaReservation {
        pub fn new() -> Result<Self, VmmFault> {
            let size = ARENA_SIZE_BYTES as usize;
            // SAFETY: This reserves inaccessible address space only. No pointer is exposed, and
            // memory is released by Drop with the same size.
            let ptr = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    size,
                    libc::PROT_NONE,
                    libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE,
                    -1,
                    0,
                )
            };

            if ptr == libc::MAP_FAILED {
                Err(VmmFault::ArenaReservation {
                    message: std::io::Error::last_os_error().to_string(),
                })
            } else {
                Ok(Self { base: ptr as usize })
            }
        }

        pub const fn logical() -> Self {
            Self { base: 0 }
        }
    }

    impl Drop for ArenaReservation {
        fn drop(&mut self) {
            if self.base == 0 {
                return;
            }
            // SAFETY: `base` came from mmap with ARENA_SIZE_BYTES in `new`.
            unsafe {
                libc::munmap(self.base as *mut libc::c_void, ARENA_SIZE_BYTES as usize);
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod arena {
    use super::VmmFault;

    #[derive(Debug)]
    pub struct ArenaReservation;

    impl ArenaReservation {
        pub const fn new() -> Result<Self, VmmFault> {
            Ok(Self)
        }

        pub const fn logical() -> Self {
            Self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ARENA_SIZE_BYTES, GuestAddress, GuestMemory, MemoryAccess, PAGE_SIZE, PagePermissions,
        VmmFault,
    };

    #[test]
    fn arena_constants_match_phase_10() {
        let memory = GuestMemory::new_logical();

        assert_eq!(memory.arena_size_bytes(), 64 * 1024 * 1024 * 1024);
        assert_eq!(ARENA_SIZE_BYTES, memory.arena_size_bytes());
    }

    #[test]
    fn maps_reads_writes_and_dumps_page() {
        let mut memory = GuestMemory::new_logical();
        memory
            .map_page(GuestAddress(0x1000), PagePermissions::READ_WRITE)
            .expect("page should map");

        memory
            .write(GuestAddress(0x1004), &[1, 2, 3, 4])
            .expect("write should succeed");
        let bytes = memory
            .read(GuestAddress(0x1004), 4)
            .expect("read should succeed");
        let dump = memory
            .debug_dump(GuestAddress(0x1004), 4)
            .expect("dump should succeed");

        assert_eq!(bytes, vec![1, 2, 3, 4]);
        assert_eq!(dump.bytes, bytes);
    }

    #[test]
    fn read_faults_on_unmapped_page() {
        let memory = GuestMemory::new_logical();
        let error = memory
            .read(GuestAddress(0x2000), 1)
            .expect_err("read should fault");

        assert_eq!(
            error,
            VmmFault::Unmapped {
                address: GuestAddress(0x2000)
            }
        );
    }

    #[test]
    fn write_faults_on_read_only_page() {
        let mut memory = GuestMemory::new_logical();
        memory
            .map_page(GuestAddress(0), PagePermissions::READ)
            .expect("page should map");

        let error = memory
            .write(GuestAddress(0), &[1])
            .expect_err("write should fault");

        assert!(matches!(error, VmmFault::Permission { .. }));
    }

    #[test]
    fn out_of_range_faults() {
        let memory = GuestMemory::new_logical();
        let error = memory
            .read(GuestAddress(ARENA_SIZE_BYTES), 1)
            .expect_err("read should fault");

        assert!(matches!(error, VmmFault::OutOfRange { .. }));
    }

    #[test]
    fn cross_page_invalid_access_faults_on_second_page() {
        let mut memory = GuestMemory::new_logical();
        memory
            .map_page(GuestAddress(0), PagePermissions::READ_WRITE)
            .expect("page should map");
        memory
            .write(GuestAddress(PAGE_SIZE - 2), &[9, 9])
            .expect("first-page seed should write");

        let error = memory
            .write(GuestAddress(PAGE_SIZE - 2), &[1, 2, 3, 4])
            .expect_err("cross-page write should fault");

        assert_eq!(
            error,
            VmmFault::Unmapped {
                address: GuestAddress(PAGE_SIZE)
            }
        );
        assert_eq!(
            memory
                .read(GuestAddress(PAGE_SIZE - 2), 2)
                .expect("seed bytes should remain readable"),
            vec![9, 9]
        );
    }

    #[test]
    fn cross_page_permission_fault_does_not_partially_write() {
        let mut memory = GuestMemory::new_logical();
        memory
            .map_page(GuestAddress(0), PagePermissions::READ_WRITE)
            .expect("first page should map");
        memory
            .map_page(GuestAddress(PAGE_SIZE), PagePermissions::READ)
            .expect("second page should map read-only");
        memory
            .write(GuestAddress(PAGE_SIZE - 2), &[7, 7])
            .expect("first-page seed should write");

        let error = memory
            .write(GuestAddress(PAGE_SIZE - 2), &[1, 2, 3, 4])
            .expect_err("cross-page write should fault on read-only page");

        assert!(matches!(
            error,
            VmmFault::Permission {
                address: GuestAddress(PAGE_SIZE),
                access: MemoryAccess::Write,
                ..
            }
        ));
        assert_eq!(
            memory
                .read(GuestAddress(PAGE_SIZE - 2), 2)
                .expect("seed bytes should remain readable"),
            vec![7, 7]
        );
    }

    #[test]
    fn maps_last_page_from_unaligned_address() {
        let mut memory = GuestMemory::new_logical();
        let last_byte = GuestAddress(ARENA_SIZE_BYTES - 1);

        memory
            .map_page(last_byte, PagePermissions::READ_WRITE)
            .expect("last page should map from an address inside it");
        memory
            .write(last_byte, &[0xAB])
            .expect("last byte should be writable");

        assert_eq!(
            memory
                .read(last_byte, 1)
                .expect("last byte should be readable"),
            vec![0xAB]
        );
    }

    #[test]
    fn unmap_removes_page() {
        let mut memory = GuestMemory::new_logical();
        memory
            .map_page(GuestAddress(0), PagePermissions::READ_WRITE)
            .expect("page should map");
        memory
            .unmap_page(GuestAddress(0))
            .expect("page should unmap");

        assert!(matches!(
            memory.read(GuestAddress(0), 1),
            Err(VmmFault::Unmapped { .. })
        ));
    }
}
