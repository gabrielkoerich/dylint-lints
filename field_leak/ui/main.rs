// A queue that works on its fields, and code elsewhere that does too
mod queue {
    pub struct RunQueue {
        pub items: Vec<u32>,
        pub head: usize,
        limit: usize,
    }

    impl RunQueue {
        pub fn new() -> Self {
            RunQueue { items: Vec::new(), head: 0, limit: 8 }
        }

        pub fn full(&self) -> bool {
            self.items.len() >= self.limit
        }
    }
}

mod sched {
    use crate::queue::RunQueue;

    pub fn admit(queue: &mut RunQueue, task: u32) {
        if !queue.full() {
            queue.items.push(task);
        }
    }

    pub fn next(queue: &mut RunQueue) -> Option<u32> {
        let task = queue.items.get(queue.head).copied();
        queue.head += 1;
        task
    }

    // A copy of a field is a read
    pub fn backlog(queue: &RunQueue) -> usize {
        queue.head
    }
}

// Plain data keeps nothing to itself, so it guards nothing
mod point {
    pub struct Point {
        pub x: i32,
        pub y: i32,
    }

    impl Point {
        pub fn norm(&self) -> i32 {
            self.x.abs() + self.y.abs()
        }
    }
}

fn shift(point: &mut point::Point) {
    point.x += 1;
    point.y += 1;
}

// A getter does no work on the fields, so nothing is guarded
mod config {
    pub struct Config {
        pub name: String,
        port: u16,
    }

    impl Config {
        pub fn new() -> Self {
            Config { name: String::new(), port: 1 }
        }

        pub fn port(&self) -> u16 {
            self.port
        }
    }
}

fn rename(config: &mut config::Config) {
    config.name.push('x');
}

// An impl block elsewhere is still inside the struct, and a shared borrow is a read
mod cache {
    pub struct Cache {
        pub entries: Vec<u8>,
        pub(super) hits: u32,
    }
}

mod cache_impl {
    impl super::cache::Cache {
        pub fn new() -> Self {
            super::cache::Cache { entries: Vec::new(), hits: 0 }
        }

        pub fn hit(&mut self) -> usize {
            self.hits += 1;
            self.entries.len()
        }
    }
}

fn peek(cache: &cache::Cache) -> &Vec<u8> {
    &cache.entries
}

mod allowed {
    pub struct Store {
        #[allow(field_leak)]
        pub rows: Vec<u8>,
        count: usize,
    }

    impl Store {
        pub fn new() -> Self {
            Store { rows: Vec::new(), count: 0 }
        }

        pub fn grow(&mut self) {
            self.count += self.rows.len();
        }
    }

    pub fn fill(store: &mut Store) {
        store.rows.push(1);
    }
}

fn main() {
    let mut queue = queue::RunQueue::new();
    sched::admit(&mut queue, 1);
    let _ = sched::next(&mut queue).unwrap_or(0) as usize + sched::backlog(&queue);
    let mut point = point::Point { x: 1, y: 2 };
    shift(&mut point);
    let _ = point.norm();
    let mut config = config::Config::new();
    rename(&mut config);
    let _ = config.port();
    let mut cache = cache::Cache::new();
    let _ = cache.hit() + peek(&cache).len();
    let mut store = allowed::Store::new();
    allowed::fill(&mut store);
    store.grow();
}
