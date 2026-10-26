use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use gpui_kit::{App, RenderImage};

pub struct Thumbs {
    images: HashMap<String, Arc<RenderImage>>,
    order: VecDeque<String>,
    pending: HashSet<String>,
    cap: usize,
}

pub fn decode(bytes: &[u8]) -> Option<RenderImage> {
    let mut rgba = image::load_from_memory(bytes).ok()?.into_rgba8();
    for px in rgba.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    Some(RenderImage::new(vec![image::Frame::new(rgba)]))
}

impl Thumbs {
    pub fn new(cap: usize) -> Self {
        Thumbs {
            images: HashMap::new(),
            order: VecDeque::new(),
            pending: HashSet::new(),
            cap,
        }
    }

    pub fn get(&self, key: &str) -> Option<Arc<RenderImage>> {
        self.images.get(key).cloned()
    }

    pub fn wants(&mut self, key: &str) -> bool {
        !self.images.contains_key(key) && self.pending.insert(key.to_string())
    }

    pub fn retry_failed(&mut self) {
        self.pending.clear();
    }

    pub fn insert(&mut self, key: String, image: Arc<RenderImage>, cx: &mut App) {
        self.pending.remove(&key);
        self.order.push_back(key.clone());
        self.images.insert(key, image);
        while self.order.len() > self.cap {
            if let Some(old) = self.order.pop_front()
                && let Some(img) = self.images.remove(&old)
            {
                cx.drop_image(img, None);
            }
        }
    }
}
