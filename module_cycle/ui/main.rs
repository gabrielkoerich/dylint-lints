// Two siblings, by paths
mod kinds {
    pub const LIMIT: usize = super::socket::BACKLOG;
    pub struct Ready;
}

mod socket {
    pub const BACKLOG: usize = 8;

    pub fn listen() -> super::kinds::Ready {
        super::kinds::Ready
    }
}

// A module and its child use each other, which makes no circle
mod outer {
    pub fn f() -> u8 {
        inner::g()
    }

    pub mod inner {
        pub fn g() -> u8 {
            super::h()
        }
    }

    fn h() -> u8 {
        1
    }
}

// Nested siblings, one using the other only by a method call and a field
mod store {
    pub mod queue {
        pub struct Queue {
            pub len: usize,
        }

        impl Queue {
            pub fn size(&self) -> usize {
                super::stats::count()
            }
        }
    }

    pub mod stats {
        pub fn count() -> usize {
            let queue = crate::make();
            queue.len + queue.size()
        }
    }
}

// Three in a ring are one report
mod ring {
    mod a {
        pub fn f() {
            super::b::g();
        }
    }

    mod b {
        pub fn g() {
            super::c::h();
        }
    }

    mod c {
        pub fn h() {
            super::a::f();
        }
    }
}

#[allow(module_cycle)]
mod quiet {
    mod a {
        pub fn f() {
            super::b::g();
        }
    }

    mod b {
        pub fn g() {
            super::a::f();
        }
    }
}

fn make() -> store::queue::Queue {
    store::queue::Queue { len: 0 }
}

fn main() {
    let _ = socket::listen();
    let _ = kinds::LIMIT + outer::f() as usize + make().size();
}
