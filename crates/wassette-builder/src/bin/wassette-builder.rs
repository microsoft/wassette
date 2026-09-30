// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

mod memory;

#[global_allocator]
static ALLOCATOR: memory::TransformAllocator = memory::TransformAllocator;

fn main() {
    if let Err(error) =
        wassette_builder::helper::main(memory::allow_vm_memory, memory::limit_transforms)
    {
        if let Some(error) = error.downcast_ref::<wassette_builder::BuildError>() {
            eprintln!("builder helper failed: {error}");
        } else {
            eprintln!("builder helper failed");
        }
        std::process::exit(1);
    }
}
