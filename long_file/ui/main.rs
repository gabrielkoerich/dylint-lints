#[path = "auxiliary/long.rs"]
mod long;

#[path = "auxiliary/short.rs"]
mod short;

#[allow(long_file)]
#[path = "auxiliary/allowed.rs"]
mod allowed;

#[path = "auxiliary/inner_allowed.rs"]
mod inner_allowed;

mod inline {
    pub fn one() -> u8 {
        1
    }
}

fn main() {
    let _ = long::sum() + short::one() + allowed::sum() + inner_allowed::sum() + inline::one();
}
