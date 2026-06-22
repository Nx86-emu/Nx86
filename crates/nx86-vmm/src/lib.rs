use std::{collections::BTreeMap, ptr::NonNull};

use serde::{Deserialize, Serialize};
use thiserror::Error;

const GIB: u64 = 1024 * 1024 * 1024;
pub const ARENA_SIZE_BYTES: u64 = 64 * GIB;
pub const PAGE_SIZE: u64 = 4096;
pub const PAGE_COUNT: usize = (ARENA_SIZE_BYTES / PAGE_SIZE) as usize;
pub const FASTMEM_READ: u8 = 1;
pub const FASTMEM_WRITE: u8 = 2;

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
    arena: arena::ArenaReservation,
    pages: BTreeMap<u64, Page>,
    fastmem_permissions: Vec<u8>,
}

/// Borrowed native fastmem metadata. The base addresses remain valid only for
/// the lifetime of the borrow and while the VMM is not structurally mutated.
#[derive(Clone, Copy, Debug)]
pub struct FastmemView<'a> {
    base: NonNull<u8>,
    permissions: &'a [u8],
}

impl FastmemView<'_> {
    #[must_use]
    pub fn base_address(self) -> usize {
        self.base.as_ptr() as usize
    }

    #[must_use]
    pub fn permissions_address(self) -> usize {
        self.permissions.as_ptr() as usize
    }

    #[must_use]
    pub fn permissions_for(self, address: GuestAddress) -> Option<u8> {
        self.permissions
            .get(page_index(address.page_base()))
            .copied()
    }
}

impl GuestMemory {
    pub fn new() -> Result<Self, VmmFault> {
        let arena = arena::ArenaReservation::new()?;
        let fastmem_permissions = if arena.base().is_some() {
            vec![0; PAGE_COUNT]
        } else {
            Vec::new()
        };
        Ok(Self {
            arena,
            pages: BTreeMap::new(),
            fastmem_permissions,
        })
    }

    pub fn new_logical() -> Self {
        Self {
            arena: arena::ArenaReservation::logical(),
            pages: BTreeMap::new(),
            fastmem_permissions: Vec::new(),
        }
    }

    #[must_use]
    pub const fn arena_size_bytes(&self) -> u64 {
        ARENA_SIZE_BYTES
    }

    /// Return the direct-memory arena and its byte-per-page eligibility table.
    /// Logical/non-native memories intentionally return `None`.
    #[must_use]
    pub fn fastmem_view(&self) -> Option<FastmemView<'_>> {
        Some(FastmemView {
            base: self.arena.base()?,
            permissions: &self.fastmem_permissions,
        })
    }

    pub fn map_page(
        &mut self,
        address: GuestAddress,
        permissions: PagePermissions,
    ) -> Result<(), VmmFault> {
        let page_base = GuestAddress(address.page_base());
        self.check_range(page_base, PAGE_SIZE as usize)?;
        if let Err(error) = self.arena.map_page(page_base.0, permissions) {
            self.pages.remove(&page_base.0);
            if let Some(entry) = self.fastmem_permissions.get_mut(page_index(page_base.0)) {
                *entry = 0;
            }
            return Err(error);
        }
        let data = if self.arena.base().is_some() {
            None
        } else {
            Some(vec![0; PAGE_SIZE as usize])
        };
        self.pages.insert(page_base.0, Page { permissions, data });
        self.set_fastmem_permissions(page_base.0, permissions);
        Ok(())
    }

    pub fn unmap_page(&mut self, address: GuestAddress) -> Result<(), VmmFault> {
        self.check_range(GuestAddress(address.page_base()), 1)?;
        let page_base = address.page_base();
        self.arena.unmap_page(page_base)?;
        self.pages.remove(&page_base);
        if let Some(entry) = self.fastmem_permissions.get_mut(page_index(page_base)) {
            *entry = 0;
        }
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

            if let Some(data) = &mut page.data {
                data[offset..offset + chunk_len].copy_from_slice(&remaining[..chunk_len]);
            } else {
                self.arena.copy_in(current, &remaining[..chunk_len])?;
            }
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

            if let Some(data) = &page.data {
                output[written..written + chunk_len]
                    .copy_from_slice(&data[offset..offset + chunk_len]);
            } else {
                self.arena
                    .copy_out(current, &mut output[written..written + chunk_len])?;
            }
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

    fn set_fastmem_permissions(&mut self, page_base: u64, permissions: PagePermissions) {
        let Some(entry) = self.fastmem_permissions.get_mut(page_index(page_base)) else {
            return;
        };
        *entry = (u8::from(permissions.read) * FASTMEM_READ)
            | (u8::from(permissions.write) * FASTMEM_WRITE);
    }
}

#[derive(Debug)]
struct Page {
    permissions: PagePermissions,
    data: Option<Vec<u8>>,
}

const fn page_index(page_base: u64) -> usize {
    (page_base / PAGE_SIZE) as usize
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
    use std::ptr::NonNull;

    use super::{ARENA_SIZE_BYTES, PagePermissions, VmmFault};

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

        pub fn base(&self) -> Option<NonNull<u8>> {
            NonNull::new(self.base as *mut u8)
        }

        pub fn map_page(
            &self,
            page_base: u64,
            permissions: PagePermissions,
        ) -> Result<(), VmmFault> {
            if self.base == 0 {
                return Ok(());
            }
            let address = self.address(page_base)?;
            // Make the page writable while it is reset, then apply guest permissions.
            self.protect(address, libc::PROT_READ | libc::PROT_WRITE)?;
            // SAFETY: `address` is one committed page inside this reservation.
            unsafe { std::ptr::write_bytes(address, 0, super::PAGE_SIZE as usize) };
            if let Err(error) = self.protect(address, host_protection(permissions)) {
                // Best-effort rollback: never leave a failed mapping writable.
                let _ = self.protect(address, libc::PROT_NONE);
                return Err(error);
            }
            Ok(())
        }

        pub fn unmap_page(&self, page_base: u64) -> Result<(), VmmFault> {
            if self.base == 0 {
                return Ok(());
            }
            let address = self.address(page_base)?;
            self.protect(address, libc::PROT_NONE)?;
            // SAFETY: the range is one page inside the live reservation.
            unsafe {
                libc::madvise(
                    address.cast::<libc::c_void>(),
                    super::PAGE_SIZE as usize,
                    libc::MADV_DONTNEED,
                );
            }
            // Revoking access is the correctness boundary. Discarding physical
            // storage is only an optimization and may be refused by the host.
            Ok(())
        }

        pub fn copy_in(&self, guest_address: u64, bytes: &[u8]) -> Result<(), VmmFault> {
            let address = self.address(guest_address)?;
            // SAFETY: GuestMemory validated the complete writable mapped range.
            unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), address, bytes.len()) };
            Ok(())
        }

        pub fn copy_out(&self, guest_address: u64, output: &mut [u8]) -> Result<(), VmmFault> {
            let address = self.address(guest_address)?;
            // SAFETY: GuestMemory validated the complete readable mapped range.
            unsafe { std::ptr::copy_nonoverlapping(address, output.as_mut_ptr(), output.len()) };
            Ok(())
        }

        fn address(&self, guest_address: u64) -> Result<*mut u8, VmmFault> {
            let offset =
                usize::try_from(guest_address).map_err(|_| VmmFault::ArenaReservation {
                    message: "guest address does not fit host pointer width".to_owned(),
                })?;
            self.base
                .checked_add(offset)
                .map(|address| address as *mut u8)
                .ok_or_else(|| VmmFault::ArenaReservation {
                    message: "guest arena address overflow".to_owned(),
                })
        }

        fn protect(&self, address: *mut u8, protection: i32) -> Result<(), VmmFault> {
            // SAFETY: the range is page-aligned and lies inside the reservation.
            let result = unsafe {
                libc::mprotect(
                    address.cast::<libc::c_void>(),
                    super::PAGE_SIZE as usize,
                    protection,
                )
            };
            if result == 0 {
                Ok(())
            } else {
                Err(last_error())
            }
        }
    }

    fn host_protection(permissions: PagePermissions) -> i32 {
        let mut protection = libc::PROT_NONE;
        if permissions.read {
            protection |= libc::PROT_READ;
        }
        if permissions.write {
            protection |= libc::PROT_WRITE;
        }
        if permissions.execute {
            protection |= libc::PROT_EXEC;
        }
        protection
    }

    fn last_error() -> VmmFault {
        VmmFault::ArenaReservation {
            message: std::io::Error::last_os_error().to_string(),
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
    use std::ptr::NonNull;

    use super::{PagePermissions, VmmFault};

    #[derive(Debug)]
    pub struct ArenaReservation;

    impl ArenaReservation {
        pub const fn new() -> Result<Self, VmmFault> {
            Ok(Self)
        }

        pub const fn logical() -> Self {
            Self
        }

        pub const fn base(&self) -> Option<NonNull<u8>> {
            None
        }

        pub const fn map_page(
            &self,
            _page_base: u64,
            _permissions: PagePermissions,
        ) -> Result<(), VmmFault> {
            Ok(())
        }

        pub const fn unmap_page(&self, _page_base: u64) -> Result<(), VmmFault> {
            Ok(())
        }

        pub const fn copy_in(&self, _guest_address: u64, _bytes: &[u8]) -> Result<(), VmmFault> {
            Ok(())
        }

        pub const fn copy_out(
            &self,
            _guest_address: u64,
            _output: &mut [u8],
        ) -> Result<(), VmmFault> {
            Ok(())
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

    #[cfg(target_os = "linux")]
    #[test]
    fn fastmem_view_tracks_page_permissions() {
        let mut memory = GuestMemory::new().expect("arena should reserve");
        memory
            .map_page(GuestAddress(0x2000), PagePermissions::READ_WRITE)
            .expect("page should map");

        let view = memory.fastmem_view().expect("native arena has fastmem");
        let flags = view
            .permissions_for(GuestAddress(0x2000))
            .expect("page is in the permission table");
        assert_eq!(flags, super::FASTMEM_READ | super::FASTMEM_WRITE);

        memory
            .unmap_page(GuestAddress(0x2000))
            .expect("page should unmap");
        let view = memory.fastmem_view().expect("arena remains live");
        let flags = view
            .permissions_for(GuestAddress(0x2000))
            .expect("page is in the permission table");
        assert_eq!(flags, 0);
    }
}
