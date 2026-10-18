//! Exact production utility entrypoint for hermetic prepared-image acceptance.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tau_ext_utils::run_stdio()
}
