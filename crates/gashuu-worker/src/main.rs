#[cfg(target_arch = "wasm32")]
mod glue;
#[cfg(target_arch = "wasm32")]
mod opfs;

fn main() {
    #[cfg(target_arch = "wasm32")]
    glue::start();
}
