//! Mapping

use mem::VirtualAddress;
use paging::{PAGE_SIZE, MappingFlags, error::MmError};
use error::KernelError;
use frame_allocator::PhysicalMemRegion;
use alloc::{vec::Vec, sync::Arc};
use utils::{check_aligned, check_nonzero_length, Splittable};
use failure::Backtrace;
use kfs_libkern;

/// A memory mapping.
/// Stores the address, the length, and the type it maps.
///
/// A mapping is guaranteed to have page aligned address and length,
/// and the length will never be zero.
///
/// If the mapping maps physical frames, we also guarantee that the
/// the virtual length of the mapping is equal to the physical length it maps.
///
/// Getting the last address of this mapping (length - 1 + address) is guaranteed to not overflow.
/// However we do not make any assumption on address + length, which falls outside of the mapping.
#[derive(Debug)]
pub struct Mapping {
    address: VirtualAddress,
    length: usize,
    mtype: MappingType,
    flags: MappingFlags,
}

/// The types that a UserSpace mapping can be in.
///
/// If it maps physical memory regions, we hold them in a Vec.
/// They will be de-allocated when this enum is dropped.
#[derive(Debug)]
pub enum MappingType {
    /// Available, nothing is stored there. Accessing to it will page fault.
    /// An allocation can use this region.
    Available,
    /// Guarded, like Available, but nothing can be allocated here.
    /// Used to implement guard pages.
    Guarded,
    /// Regular, a region known only by this process.
    /// Access rights are stored in Mapping.mtype.
    Regular(Vec<PhysicalMemRegion>),
//    Stack(Vec<PhysicalMemRegion>),
    /// Shared, a region that can be mapped in multiple processes.
    /// Access rights are stored in Mapping.mtype.
    Shared(Arc<Vec<PhysicalMemRegion>>),
    /// SystemReserved, used to denote the KernelLand and other similar regions that the user
    /// cannot access, and shouldn't know anything more about.
    /// Cannot be unmapped, nor modified in any way.
    SystemReserved
}

impl<'a> From<&'a MappingType> for kfs_libkern::MemoryType {
    fn from(ty: &'a MappingType) -> kfs_libkern::MemoryType {
        match ty {
            // TODO: Extend MappingType to cover all MemoryTypes
            // BODY: Currently, MappingType only covers a very limited view of the mappings.
            // It should have the ability to understand all the various kind of memory allocations,
            // such as "Heap", "CodeMemory", "SharedMemory", "TransferMemory", etc...

            MappingType::Available => kfs_libkern::MemoryType::Unmapped,
            MappingType::Guarded => kfs_libkern::MemoryType::Reserved,
            MappingType::Regular(_) => kfs_libkern::MemoryType::Normal,
            MappingType::Shared(_) => kfs_libkern::MemoryType::SharedMemory,
            MappingType::SystemReserved => kfs_libkern::MemoryType::Reserved,
        }
    }
}

impl Mapping {
    /// Tries to construct a regular mapping.
    ///
    /// # Error
    ///
    /// Returns an Error if `address` + `frames`'s length would overflow.
    /// Returns an Error if `address` is not page aligned.
    /// Returns an Error if `length` is 0.
    pub fn new_regular(address: VirtualAddress, frames: Vec<PhysicalMemRegion>, flags: MappingFlags) -> Result<Mapping, KernelError> {
        check_aligned(address.addr(), PAGE_SIZE)?;
        let length = frames.iter().flatten().count() * PAGE_SIZE;
        check_nonzero_length(length)?;
        address.checked_add(length - 1)?;
        Ok(Mapping { address, length, mtype: MappingType::Regular(frames), flags })
    }

    /// Tries to construct a shared mapping.
    ///
    /// # Error
    ///
    /// Returns an Error if `address` + `frames`'s length would overflow.
    /// Returns an Error if `address` is not page aligned.
    /// Returns an Error if `length` is 0.
    pub fn new_shared(address: VirtualAddress, frames: Arc<Vec<PhysicalMemRegion>>, flags: MappingFlags) -> Result<Mapping, KernelError> {
        check_aligned(address.addr(), PAGE_SIZE)?;
        let length = frames.iter().flatten().count() * PAGE_SIZE;
        check_nonzero_length(length)?;
        address.checked_add(length - 1)?;
        Ok(Mapping { address, length, mtype: MappingType::Shared(frames), flags })
    }

    /// Tries to construct a guarded mapping.
    ///
    /// # Error
    ///
    /// Returns an Error if `address` + `length` would overflow.
    /// Returns an Error if `address` or `length` is not page aligned.
    /// Returns an Error if `length` is 0.
    pub fn new_guard(address: VirtualAddress, length: usize) -> Result<Mapping, KernelError> {
        check_aligned(address.addr(), PAGE_SIZE)?;
        check_aligned(length, PAGE_SIZE)?;
        check_nonzero_length(length)?;
        address.checked_add(length - 1)?;
        Ok(Mapping { address, length, mtype: MappingType::Guarded, flags: MappingFlags::empty() })
    }

    /// Tries to construct an available mapping.
    ///
    /// # Error
    ///
    /// Returns an Error if `address` + `length` would overflow.
    /// Returns an Error if `address` or `length` is not page aligned.
    /// Returns an Error if `length` is 0.
    pub fn new_available(address: VirtualAddress, length: usize) -> Result<Mapping, KernelError> {
        check_aligned(address.addr(), PAGE_SIZE)?;
        check_aligned(length, PAGE_SIZE)?;
        check_nonzero_length(length)?;
        address.checked_add(length - 1)?;
        Ok(Mapping { address, length, mtype: MappingType::Available, flags: MappingFlags::empty() })
    }

    /// Tries to construct a system reserved mapping.
    ///
    /// # Error
    ///
    /// Returns an Error if `address` + `length` would overflow.
    /// Returns an Error if `address` or `length` is not page aligned.
    /// Returns an Error if `length` is 0.
    pub fn new_system_reserved(address: VirtualAddress, length: usize) -> Result<Mapping, KernelError> {
        check_aligned(address.addr(), PAGE_SIZE)?;
        check_aligned(length, PAGE_SIZE)?;
        check_nonzero_length(length)?;
        address.checked_add(length - 1)?;
        Ok(Mapping { address, length, mtype: MappingType::SystemReserved, flags: MappingFlags::empty() })
    }

    /// Returns the address of this mapping.
    ///
    /// Because we make guarantees about a mapping being always valid, this field cannot be public.
    pub fn address(&self) -> VirtualAddress { self.address }

    /// Returns the address of this mapping.
    ///
    /// Because we make guarantees about a mapping being always valid, this field cannot be public.
    pub fn length(&self) -> usize { self.length }

    /// Returns a reference to the type of this mapping.
    ///
    /// Because we make guarantees about a mapping being always valid, this field cannot be public.
    pub fn mtype_ref(&self) -> &MappingType { &self.mtype }

    /// Returns the type of this mapping.
    ///
    /// Because we make guarantees about a mapping being always valid, this field cannot be public.
    pub fn mtype(self) -> MappingType { self.mtype }

    /// Returns the type of this mapping.
    ///
    /// Because we make guarantees about a mapping being always valid, this field cannot be public.
    pub fn flags(&self) -> MappingFlags { self.flags }
}

impl Splittable for Mapping {
    /// Splits a mapping at a given offset.
    ///
    /// Because it is reference counted, a Shared mapping cannot be splitted.
    ///
    /// # Error
    ///
    /// * SharedMapping if it's a shared mapping.
    /// * InvalidMapping if it's a system reserved mapping.
    fn split_at(&mut self, offset: usize) -> Result<Option<Self>, KernelError> {
        check_aligned(offset, PAGE_SIZE)?;
        if offset == 0 || offset >= self.length { return Ok(None) };
        let right = Mapping {
            address: self.address + offset,
            length: self.length - offset,
            flags: self.flags,
            mtype: match &mut self.mtype {
                MappingType::Available => MappingType::Available,
                MappingType::Guarded => MappingType::Guarded,
                MappingType::Regular(ref mut frames) => MappingType::Regular(frames.split_at(offset)?.unwrap()),
            //    MappingType::Stack(ref mut frames) => MappingType::Stack(frames.split_at(offset)?.unwrap()),
                MappingType::Shared(_) => return Err(KernelError::MmError(
                                                       MmError::SharedMapping { backtrace: Backtrace::new() })),
                MappingType::SystemReserved => return Err(KernelError::MmError(
                                                       MmError::InvalidMapping { backtrace: Backtrace::new() })),
            },
        };
        // split succeeded, now modify left part
        self.length = offset;
        Ok(Some(right))
    }
}


#[cfg(test)]
mod test {
    use super::Mapping;
    use super::MappingFlags;
    use mem::{VirtualAddress, PhysicalAddress};
    use paging::PAGE_SIZE;
    use frame_allocator::{PhysicalMemRegion, FrameAllocator, FrameAllocatorTrait};
    use std::sync::Arc;

    /// Applies the same tests to guard, available and system_reserved.
    macro_rules! test_empty_mapping {
        ($($x:ident),*) => {
            mashup! {
                $(
                m["new_" $x] = new_ $x;
                m["mapping_ok_" $x] = $x _mapping_ok;
                m["mapping_zero_length_" $x] = $x _mapping_zero_length;
                m["mapping_non_aligned_addr_" $x] = $x _mapping_non_aligned_addr;
                m["mapping_non_aligned_length_" $x] = $x _mapping_non_aligned_length;
                m["mapping_length_threshold_" $x] = $x _mapping_length_threshold;
                m["mapping_length_overflow_" $x] = $x _mapping_length_overflow;
                )*
            }
            m! {
                $(
                #[test]
                fn "mapping_ok_" $x () {
                    Mapping:: "new_" $x (VirtualAddress(0x40000000), 3 * PAGE_SIZE).unwrap();
                }

                #[test]
                fn "mapping_zero_length_" $x () {
                    Mapping:: "new_" $x (VirtualAddress(0x40000000), 0).unwrap_err();
                }

                #[test]
                fn "mapping_non_aligned_addr_" $x () {
                    Mapping::"new_" $x (VirtualAddress(0x40000007), 3 * PAGE_SIZE).unwrap_err();
                }

                #[test]
                fn "mapping_non_aligned_length_" $x () {
                    Mapping::"new_" $x (VirtualAddress(0x40000000), 3).unwrap_err();
                }

                #[test]
                fn "mapping_length_threshold_" $x () {
                    Mapping::"new_" $x (VirtualAddress(usize::max_value() - 2 * PAGE_SIZE + 1), 2 * PAGE_SIZE).unwrap();
                }

                #[test]
                fn "mapping_length_overflow_" $x () {
                    Mapping::"new_" $x (VirtualAddress(usize::max_value() - 2 * PAGE_SIZE + 1), 3 * PAGE_SIZE).unwrap_err();
                }
                )*
            }
        }
    }

    test_empty_mapping!(guard, available, system_reserved);

    #[test]
    fn mapping_regular_ok() {
        let _f = ::frame_allocator::init();
        let frames = FrameAllocator::allocate_frames_fragmented(2 * PAGE_SIZE).unwrap();
        let flags = MappingFlags::u_rw();
        let _mapping = Mapping::new_regular(VirtualAddress(0x40000000), frames, flags).unwrap();
    }

    #[test]
    fn mapping_shared_ok() {
        let _f = ::frame_allocator::init();
        let frames = Arc::new(FrameAllocator::allocate_frames_fragmented(2 * PAGE_SIZE).unwrap());
        let flags = MappingFlags::u_rw();
        let _mapping = Mapping::new_shared(VirtualAddress(0x40000000), frames, flags).unwrap();
    }

    #[test]
    fn mapping_regular_empty_vec() {
        let _f = ::frame_allocator::init();
        let frames = Vec::new();
        let flags = MappingFlags::u_rw();
        let _mapping_err = Mapping::new_regular(VirtualAddress(0x40000000), frames, flags).unwrap_err();
    }

    #[test]
    fn mapping_shared_empty_vec() {
        let _f = ::frame_allocator::init();
        let frames = Arc::new(Vec::new());
        let flags = MappingFlags::u_rw();
        let _mapping_err = Mapping::new_shared(VirtualAddress(0x40000000), frames, flags).unwrap_err();
    }

    #[test]
    fn mapping_regular_zero_sized_region() {
        let _f = ::frame_allocator::init();
        let region = unsafe { PhysicalMemRegion::reconstruct_no_dealloc(PhysicalAddress(PAGE_SIZE), 0) };
        let frames = vec![region];
        let flags = MappingFlags::u_rw();
        let _mapping_err = Mapping::new_regular(VirtualAddress(0x40000000), frames, flags).unwrap_err();
    }

    #[test]
    fn mapping_regular_zero_sized_regions() {
        let _f = ::frame_allocator::init();
        let region1 = unsafe { PhysicalMemRegion::reconstruct_no_dealloc(PhysicalAddress(PAGE_SIZE), 0) };
        let region2 = unsafe { PhysicalMemRegion::reconstruct_no_dealloc(PhysicalAddress(PAGE_SIZE), 0) };
        let frames = vec![region1, region2];
        let flags = MappingFlags::u_rw();
        let _mapping_err = Mapping::new_regular(VirtualAddress(0x40000000), frames, flags).unwrap_err();
    }

    #[test]
    fn mapping_regular_unaligned_addr() {
        let _f = ::frame_allocator::init();
        let frames = FrameAllocator::allocate_frames_fragmented(2 * PAGE_SIZE).unwrap();
        let flags = MappingFlags::u_rw();
        let _mapping_err = Mapping::new_regular(VirtualAddress(0x40000007), frames, flags).unwrap_err();
    }

    #[test]
    fn mapping_shared_unaligned_addr() {
        let _f = ::frame_allocator::init();
        let frames = Arc::new(FrameAllocator::allocate_frames_fragmented(2 * PAGE_SIZE).unwrap());
        let flags = MappingFlags::u_rw();
        let _mapping_err = Mapping::new_shared(VirtualAddress(0x40000007), frames, flags).unwrap_err();
    }


    #[test]
    #[should_panic]
    fn mapping_regular_unaligned_len() {
        let _f = ::frame_allocator::init();
        let frames = FrameAllocator::allocate_frames_fragmented(2 * PAGE_SIZE + 7).unwrap();
        let flags = MappingFlags::u_rw();
        let _mapping = Mapping::new_regular(VirtualAddress(0x40000000), frames, flags).unwrap();
    }

    #[test]
    #[should_panic]
    fn mapping_shared_unaligned_len() {
        let _f = ::frame_allocator::init();
        let frames = Arc::new(FrameAllocator::allocate_frames_fragmented(2 * PAGE_SIZE + 7).unwrap());
        let flags = MappingFlags::u_rw();
        let _mapping = Mapping::new_shared(VirtualAddress(0x40000000), frames, flags).unwrap();
    }

    #[test]
    fn mapping_regular_threshold() {
        let _f = ::frame_allocator::init();
        let frames = FrameAllocator::allocate_frames_fragmented(2 * PAGE_SIZE).unwrap();
        let flags = MappingFlags::u_rw();
        let _mapping = Mapping::new_regular(VirtualAddress(usize::max_value() - 2 * PAGE_SIZE + 1), frames, flags).unwrap();
    }

    #[test]
    fn mapping_shared_threshold() {
        let _f = ::frame_allocator::init();
        let frames = Arc::new(FrameAllocator::allocate_frames_fragmented(2 * PAGE_SIZE).unwrap());
        let flags = MappingFlags::u_rw();
        let _mapping = Mapping::new_shared(VirtualAddress(usize::max_value() - 2 * PAGE_SIZE + 1), frames, flags).unwrap();
    }

    #[test]
    fn mapping_regular_overflow() {
        let _f = ::frame_allocator::init();
        let frames = FrameAllocator::allocate_frames_fragmented(2 * PAGE_SIZE).unwrap();
        let flags = MappingFlags::u_rw();
        let _mapping_err = Mapping::new_regular(VirtualAddress(usize::max_value() - 2 * PAGE_SIZE), frames, flags).unwrap_err();
    }

    #[test]
    fn mapping_shared_overflow() {
        let _f = ::frame_allocator::init();
        let frames = Arc::new(FrameAllocator::allocate_frames_fragmented(2 * PAGE_SIZE).unwrap());
        let flags = MappingFlags::u_rw();
        let _mapping_err = Mapping::new_shared(VirtualAddress(usize::max_value() - 2 * PAGE_SIZE), frames, flags).unwrap_err();
    }

    // TODO: Test Splittable<Mapping>
    // BODY: Write some tests for splitting a Mapping,
    // BODY: as I am really not confident it works as expected.
}
