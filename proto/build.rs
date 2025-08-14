fn main() -> Result<(), Box<dyn std::error::Error>> {
    prost_build::compile_protos(&["src/messages.proto"], &["src/"])?;
    Ok(())
}