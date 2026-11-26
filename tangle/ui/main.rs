// A type and its child module's type know each other, which no module cycle shows
mod kernel {
    pub struct Tasks;

    impl Tasks {
        pub fn wake(&self) {}

        pub fn sleep(&self, timers: &timers::Timers) {
            timers.fire(self);
        }
    }

    pub mod timers {
        pub struct Timers;

        impl Timers {
            pub fn fire(&self, tasks: &super::Tasks) {
                tasks.wake();
            }
        }
    }
}

// A circle inside one module is fine while it is small
mod parse {
    pub fn expr(depth: u8) -> u8 {
        if depth == 0 { 0 } else { term(depth - 1) }
    }

    fn term(depth: u8) -> u8 {
        expr(depth)
    }
}

// An impl block in another module still belongs to its type, and is where the circle is written
mod shapes {
    pub struct Circle;
    pub struct Square;

    impl Square {
        pub fn area(&self) -> u8 {
            Circle.radius()
        }
    }
}

mod impls {
    impl super::shapes::Circle {
        pub fn radius(&self) -> u8 {
            1
        }

        pub fn fit(&self, square: &super::shapes::Square) -> u8 {
            square.area()
        }
    }
}

mod eight {
    pub fn a(n: u8) -> u8 { if n == 0 { 0 } else { b(n - 1) } }
    fn b(n: u8) -> u8 { c(n) }
    fn c(n: u8) -> u8 { d(n) }
    fn d(n: u8) -> u8 { e(n) }
    fn e(n: u8) -> u8 { f(n) }
    fn f(n: u8) -> u8 { g(n) }
    fn g(n: u8) -> u8 { h(n) }
    fn h(n: u8) -> u8 { a(n) }
}

mod accepted {
    pub fn a() {
        inner::b();
    }

    #[allow(tangle)]
    pub mod inner {
        pub fn b() {
            super::a();
        }
    }
}

fn main() {
    kernel::Tasks.sleep(&kernel::timers::Timers);
    let _ = parse::expr(3) + shapes::Circle.fit(&shapes::Square) + eight::a(2);
    accepted::a();
}
