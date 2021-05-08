use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ops::{Index, IndexMut};
use std::rc::Rc;

use crate::disk_manager::*;

pub enum Error {
    NoFreeBuffer,
    IoError(std::io::Error),
}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::IoError(e)
    }
}

#[derive(Clone, Copy)]
pub struct BufferId(usize);

pub type Page = [u8; PAGE_SIZE];

pub struct Buffer {
    pub page_id: PageId,
    pub page: RefCell<Page>,
    pub is_dirty: Cell<bool>,
}

pub struct Frame {
    usage_count: u64,
    buffer: Rc<Buffer>,
}

pub struct BufferPool {
    buffers: Vec<Frame>,
    next_victim_id: BufferId,
}
impl BufferPool {
    fn size(&self) -> usize {
        self.buffers.len()
    }
    /// 使っていい BufferId を返す
    fn evict(&mut self) -> Option<BufferId> {
        let pool_size = self.size();
        let mut consecutive_pinned = 0;

        // 捨てて良い BufferId を探す
        let victim_id = loop {
            let next_victim_id = self.next_victim_id;
            let frame = &mut self[next_victim_id];
            if frame.usage_count == 0 {
                break self.next_victim_id;
            }
            if Rc::get_mut(&mut frame.buffer).is_some() {
                // 掴めるということは使用していない
                frame.usage_count -= 1;
                consecutive_pinned = 0;
            } else {
                // 掴めない
                consecutive_pinned += 1;
                if consecutive_pinned >= pool_size {
                    return None;
                }
            }
            self.next_victim_id = self.increment_id(self.next_victim_id);
        };
        Some(victim_id)
    }
    fn increment_id(&self, buffer_id: BufferId) -> BufferId {
        BufferId((buffer_id.0 + 1) % self.size())
    }
}

impl Index<BufferId> for BufferPool {
    type Output = Frame;

    fn index(&self, index: BufferId) -> &Self::Output {
        &self.buffers[index.0]
    }
}

impl IndexMut<BufferId> for BufferPool {
    fn index_mut(&mut self, index: BufferId) -> &mut Self::Output {
        &mut self.buffers[index.0]
    }
}

pub struct BufferPoolManager {
    disk: DiskManager,
    pool: BufferPool,
    page_table: HashMap<PageId, BufferId>,
}

impl BufferPoolManager {
    pub fn fetch_page(&mut self, page_id: PageId) -> Result<Rc<Buffer>, Error> {
        if let Some(&buffer_id) = self.page_table.get(&page_id) {
            let frame = &mut self.pool[buffer_id];
            frame.usage_count += 1;
            return Ok(frame.buffer.clone());
        }
        // 使っていいバッファを決定する
        let buffer_id = self.pool.evict().ok_or(Error::NoFreeBuffer)?;
        let frame = &mut self.pool[buffer_id];
        let evict_page_id = frame.buffer.page_id;
        {
            let buffer = Rc::get_mut(&mut frame.buffer).unwrap();
            // 変化しているならば、ディスクの内容が古いので書き込む
            if buffer.is_dirty.get() {
                self.disk
                    .write_page_data(evict_page_id, buffer.page.get_mut())?;
            }
            buffer.page_id = page_id;
            buffer.is_dirty.set(false);
            // ページを読み出す
            self.disk.read_page_data(page_id, buffer.page.get_mut())?;
            frame.usage_count = 1;
        }
        // ページテーブルを更新する
        let page = Rc::clone(&frame.buffer);
        self.page_table.remove(&evict_page_id);
        self.page_table.insert(page_id, buffer_id);
        Ok(page)
    }
}
