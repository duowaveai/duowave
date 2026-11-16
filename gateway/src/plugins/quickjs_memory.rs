//! A plugin's QuickJS memory in one block of its own (docs/plans/plugins.md, «Память и место на колонке»).
//!
//! Before 16.11.2026 QuickJS took its memory from the C library's malloc, among the gateway's own: on the station
//! (musl's mallocng) every load and unload of a plugin left the shared heap a little more broken up, and the pages it
//! kept never went back to the system. A Lite 2 whose apps were loaded and unloaded through the night — events, the
//! schedules, the unload after PLUGINS_IDLE_UNLOAD_S — held 72 MB of its own memory by the morning, and the apps'
//! budget fell to 0 MB. Measured in a container with the station's build (16.11): thirty rounds of the eleven
//! built-in apps loaded and unloaded took its own memory from 15 to 64 MB while what it held in use stayed at 5 MB;
//! with the block, from 14 to 26 MB (what is left is the plugin's thread and its tokio runtime, made at each load).
//!
//! So each runtime gets one block, sized from the plugin's memory limit, and a TLSF allocator inside it (rlsf):
//! QuickJS's allocations and frees stay inside the block, and when the plugin unloads the block goes back whole —
//! musl and the other C libraries give a block this big its own mapping, unmapped at its free. The pages of the block
//! count only once QuickJS touches them.
use std::alloc::Layout;
use std::ptr::NonNull;

use rlsf::Tlsf;

/// Every allocation starts with its requested size, so QuickJS can ask a pointer's size (`usable_size` has no
/// `self`); 16 bytes keep what follows aligned for any value QuickJS stores.
const HEADER: usize = 16;
const ALIGN: usize = 16;
/// The block's alignment: a page.
const PAGE: usize = 4096;
/// Room on top of the limit: the headers, TLSF's own and the gaps between blocks. QuickJS stops at its limit (it
/// counts what it asked for), so with this much over it the block itself never runs out first.
const SLACK_MB: u64 = 1;

/// FLLEN 28: blocks up to 8 GB, more than any limit; SLLEN 16 for a finer fit.
type Pool = Tlsf<'static, u32, u16, 28, 16>;

/// One runtime's memory. Not Send: it lives and dies on the plugin's thread, with its runtime.
pub struct BlockAllocator {
    block: NonNull<u8>,
    layout: Layout,
    pool: Box<Pool>,
}

impl BlockAllocator {
    /// A block for a runtime limited to `limit_mb`: twice the limit (QuickJS's many small allocations carry a header
    /// and TLSF's rounding each) and a megabyte. None when the system has no room for it.
    pub fn for_limit(limit_mb: u64) -> Option<BlockAllocator> {
        let bytes = limit_mb.max(1).saturating_mul(2).saturating_add(SLACK_MB).saturating_mul(1024 * 1024);
        BlockAllocator::with_size(usize::try_from(bytes).ok()?)
    }

    fn with_size(size: usize) -> Option<BlockAllocator> {
        let layout = Layout::from_size_align(size, PAGE).ok()?;
        // SAFETY: a non-zero size; the block is freed with this same layout in Drop.
        let block = NonNull::new(unsafe { std::alloc::alloc(layout) })?;
        let mut pool = Box::new(Pool::new());
        // SAFETY: the block is ours until Drop, which outlives the pool's every use (the pool is dropped with it).
        unsafe { pool.insert_free_block_ptr(NonNull::slice_from_raw_parts(block, size)) };
        Some(BlockAllocator { block, layout, pool })
    }

    fn layout_for(size: usize) -> Option<Layout> {
        Layout::from_size_align(size.checked_add(HEADER)?, ALIGN).ok()
    }
}

impl Drop for BlockAllocator {
    fn drop(&mut self) {
        // SAFETY: allocated in `with_size` with this layout. rquickjs drops the allocator after JS_FreeRuntime.
        unsafe { std::alloc::dealloc(self.block.as_ptr(), self.layout) };
    }
}

// SAFETY: every pointer handed out is HEADER past the start of a TLSF block of at least size + HEADER bytes,
// 16-aligned (at least usize's alignment, as rquickjs asks), and `usable_size` reads the size written there.
unsafe impl rquickjs::allocator::Allocator for BlockAllocator {
    fn alloc(&mut self, size: usize) -> *mut u8 {
        let Some(layout) = BlockAllocator::layout_for(size) else { return std::ptr::null_mut() };
        let Some(start) = self.pool.allocate(layout) else { return std::ptr::null_mut() };
        // SAFETY: the block has room for the header and `size` bytes.
        unsafe {
            start.as_ptr().cast::<usize>().write(size);
            start.as_ptr().add(HEADER)
        }
    }

    fn calloc(&mut self, count: usize, size: usize) -> *mut u8 {
        let Some(total) = count.checked_mul(size) else { return std::ptr::null_mut() };
        let pointer = self.alloc(total);
        if !pointer.is_null() {
            // SAFETY: `total` bytes were just allocated there.
            unsafe { std::ptr::write_bytes(pointer, 0, total) };
        }
        pointer
    }

    unsafe fn dealloc(&mut self, pointer: *mut u8) {
        // SAFETY: the caller hands back a pointer of this allocator; its block starts HEADER before it.
        unsafe { self.pool.deallocate(NonNull::new_unchecked(pointer.sub(HEADER)), ALIGN) };
    }

    unsafe fn realloc(&mut self, pointer: *mut u8, new_size: usize) -> *mut u8 {
        if pointer.is_null() {
            return self.alloc(new_size);
        }
        let Some(layout) = BlockAllocator::layout_for(new_size) else { return std::ptr::null_mut() };
        // SAFETY: a pointer of this allocator, allocated with ALIGN; on failure the old block is left as it was.
        unsafe {
            let start = NonNull::new_unchecked(pointer.sub(HEADER));
            match self.pool.reallocate(start, layout) {
                Some(moved) => {
                    moved.as_ptr().cast::<usize>().write(new_size);
                    moved.as_ptr().add(HEADER)
                }
                None => std::ptr::null_mut(),
            }
        }
    }

    unsafe fn usable_size(pointer: *mut u8) -> usize {
        // SAFETY: a pointer of this allocator: its size is HEADER before it.
        unsafe { pointer.sub(HEADER).cast::<usize>().read() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rquickjs::allocator::Allocator;

    #[test]
    fn allocates_reuses_and_grows_inside_its_block() {
        let mut memory = BlockAllocator::with_size(256 * 1024).unwrap();
        let first = memory.alloc(100);
        assert!(!first.is_null());
        assert_eq!(first as usize % ALIGN, 0);
        assert_eq!(unsafe { BlockAllocator::usable_size(first) }, 100);
        unsafe { std::ptr::write_bytes(first, 7, 100) };
        let grown = unsafe { memory.realloc(first, 5000) };
        assert!(!grown.is_null());
        assert_eq!(unsafe { BlockAllocator::usable_size(grown) }, 5000);
        assert!(unsafe { std::slice::from_raw_parts(grown, 100) }.iter().all(|byte| *byte == 7), "kept its bytes");
        let zeroed = memory.calloc(10, 30);
        assert!(unsafe { std::slice::from_raw_parts(zeroed, 300) }.iter().all(|byte| *byte == 0));
        unsafe {
            memory.dealloc(grown);
            memory.dealloc(zeroed);
        }
        // What was freed is used again: the block never runs out over many rounds.
        for _ in 0..10_000 {
            let pointer = memory.alloc(4000);
            assert!(!pointer.is_null());
            unsafe { memory.dealloc(pointer) };
        }
    }

    #[test]
    fn says_no_past_its_block_rather_than_taking_more() {
        let mut memory = BlockAllocator::with_size(64 * 1024).unwrap();
        assert!(memory.alloc(1024 * 1024).is_null());
        assert!(memory.calloc(usize::MAX, 2).is_null());
        let small = memory.alloc(16);
        assert!(!small.is_null());
        assert!(unsafe { memory.realloc(small, 1024 * 1024) }.is_null(), "a refused growth leaves the old block");
        assert_eq!(unsafe { BlockAllocator::usable_size(small) }, 16);
    }

    #[test]
    fn a_runtime_lives_in_its_block_and_stops_at_its_limit() {
        let engine = rquickjs::Runtime::new_with_alloc(BlockAllocator::for_limit(2).unwrap()).unwrap();
        engine.set_memory_limit(2 * 1024 * 1024);
        let context = rquickjs::Context::full(&engine).unwrap();
        context.with(|ctx| {
            let sum: i32 = ctx.eval("let s = 0; for (let i = 0; i < 1000; i++) s += [i].length; s").unwrap();
            assert_eq!(sum, 1000);
            let runaway = ctx.eval::<(), _>("const a = []; while (true) a.push('x'.repeat(1000) + a.length);");
            assert!(runaway.is_err(), "out of memory at its limit, not past the block");
        });
    }
}
