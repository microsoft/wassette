// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

#[doc(hidden)]
#[allow(non_snake_case)]
pub unsafe fn _export_list_directory_cabi<T: Guest>(
    arg0: *mut u8,
    arg1: usize,
) -> *mut u8 {
    #[cfg(target_arch = "wasm32")] _rt::run_ctors_once();
    let len0 = arg1;
    let bytes0 = _rt::Vec::from_raw_parts(arg0.cast(), len0, len0);
    let result1 = T::list_directory(_rt::string_lift(bytes0));
    let ptr2 = _RET_AREA.0.as_mut_ptr().cast::<u8>();
    match result1 {
        Ok(e) => {
            *ptr2.add(0).cast::<u8>() = (0i32) as u8;
            let vec4 = e;
            let len4 = vec4.len();
            let layout4 = _rt::alloc::Layout::from_size_align_unchecked(
                vec4.len() * 8,
                4,
            );
            let result4 = if layout4.size() != 0 {
                let ptr = _rt::alloc::alloc(layout4).cast::<u8>();
                if ptr.is_null() {
                    _rt::alloc::handle_alloc_error(layout4);
                }
                ptr
            } else {
                { ::core::ptr::null_mut() }
            };
            for (i, e) in vec4.into_iter().enumerate() {
                let base = result4.add(i * 8);
                {
                    let vec3 = (e.into_bytes()).into_boxed_slice();
                    let ptr3 = vec3.as_ptr().cast::<u8>();
                    let len3 = vec3.len();
                    ::core::mem::forget(vec3);
                    *base.add(4).cast::<usize>() = len3;
                    *base.add(0).cast::<*mut u8>() = ptr3.cast_mut();
                }
            }
            *ptr2.add(8).cast::<usize>() = len4;
            *ptr2.add(4).cast::<*mut u8>() = result4;
        }
        Err(e) => {
            *ptr2.add(0).cast::<u8>() = (1i32) as u8;
            let vec5 = (e.into_bytes()).into_boxed_slice();
            let ptr5 = vec5.as_ptr().cast::<u8>();
            let len5 = vec5.len();
            ::core::mem::forget(vec5);
            *ptr2.add(8).cast::<usize>() = len5;
            *ptr2.add(4).cast::<*mut u8>() = ptr5.cast_mut();
        }
    };
    ptr2
}
#[doc(hidden)]
#[allow(non_snake_case)]
pub unsafe fn __post_return_list_directory<T: Guest>(arg0: *mut u8) {
    let l0 = i32::from(*arg0.add(0).cast::<u8>());
    match l0 {
        0 => {
            let l1 = *arg0.add(4).cast::<*mut u8>();
            let l2 = *arg0.add(8).cast::<usize>();
            let base5 = l1;
            let len5 = l2;
            for i in 0..len5 {
                let base = base5.add(i * 8);
                {
                    let l3 = *base.add(0).cast::<*mut u8>();
                    let l4 = *base.add(4).cast::<usize>();
                    _rt::cabi_dealloc(l3, l4, 1);
                }
            }
            _rt::cabi_dealloc(base5, len5 * 8, 4);
        }
        _ => {
            let l6 = *arg0.add(4).cast::<*mut u8>();
            let l7 = *arg0.add(8).cast::<usize>();
            _rt::cabi_dealloc(l6, l7, 1);
        }
    }
}
#[doc(hidden)]
#[allow(non_snake_case)]
pub unsafe fn _export_read_file_cabi<T: Guest>(arg0: *mut u8, arg1: usize) -> *mut u8 {
    #[cfg(target_arch = "wasm32")] _rt::run_ctors_once();
    let len0 = arg1;
    let bytes0 = _rt::Vec::from_raw_parts(arg0.cast(), len0, len0);
    let result1 = T::read_file(_rt::string_lift(bytes0));
    let ptr2 = _RET_AREA.0.as_mut_ptr().cast::<u8>();
    match result1 {
        Ok(e) => {
            *ptr2.add(0).cast::<u8>() = (0i32) as u8;
            let vec3 = (e.into_bytes()).into_boxed_slice();
            let ptr3 = vec3.as_ptr().cast::<u8>();
            let len3 = vec3.len();
            ::core::mem::forget(vec3);
            *ptr2.add(8).cast::<usize>() = len3;
            *ptr2.add(4).cast::<*mut u8>() = ptr3.cast_mut();
        }
        Err(e) => {
            *ptr2.add(0).cast::<u8>() = (1i32) as u8;
            let vec4 = (e.into_bytes()).into_boxed_slice();
            let ptr4 = vec4.as_ptr().cast::<u8>();
            let len4 = vec4.len();
            ::core::mem::forget(vec4);
            *ptr2.add(8).cast::<usize>() = len4;
            *ptr2.add(4).cast::<*mut u8>() = ptr4.cast_mut();
        }
    };
    ptr2
}
#[doc(hidden)]
#[allow(non_snake_case)]
pub unsafe fn __post_return_read_file<T: Guest>(arg0: *mut u8) {
    let l0 = i32::from(*arg0.add(0).cast::<u8>());
    match l0 {
        0 => {
            let l1 = *arg0.add(4).cast::<*mut u8>();
            let l2 = *arg0.add(8).cast::<usize>();
            _rt::cabi_dealloc(l1, l2, 1);
        }
        _ => {
            let l3 = *arg0.add(4).cast::<*mut u8>();
            let l4 = *arg0.add(8).cast::<usize>();
            _rt::cabi_dealloc(l3, l4, 1);
        }
    }
}
#[doc(hidden)]
#[allow(non_snake_case)]
pub unsafe fn _export_search_file_cabi<T: Guest>(
    arg0: *mut u8,
    arg1: usize,
    arg2: *mut u8,
    arg3: usize,
) -> *mut u8 {
    #[cfg(target_arch = "wasm32")] _rt::run_ctors_once();
    let len0 = arg1;
    let bytes0 = _rt::Vec::from_raw_parts(arg0.cast(), len0, len0);
    let len1 = arg3;
    let bytes1 = _rt::Vec::from_raw_parts(arg2.cast(), len1, len1);
    let result2 = T::search_file(_rt::string_lift(bytes0), _rt::string_lift(bytes1));
    let ptr3 = _RET_AREA.0.as_mut_ptr().cast::<u8>();
    match result2 {
        Ok(e) => {
            *ptr3.add(0).cast::<u8>() = (0i32) as u8;
            let vec4 = (e.into_bytes()).into_boxed_slice();
            let ptr4 = vec4.as_ptr().cast::<u8>();
            let len4 = vec4.len();
            ::core::mem::forget(vec4);
            *ptr3.add(8).cast::<usize>() = len4;
            *ptr3.add(4).cast::<*mut u8>() = ptr4.cast_mut();
        }
        Err(e) => {
            *ptr3.add(0).cast::<u8>() = (1i32) as u8;
            let vec5 = (e.into_bytes()).into_boxed_slice();
            let ptr5 = vec5.as_ptr().cast::<u8>();
            let len5 = vec5.len();
            ::core::mem::forget(vec5);
            *ptr3.add(8).cast::<usize>() = len5;
            *ptr3.add(4).cast::<*mut u8>() = ptr5.cast_mut();
        }
    };
    ptr3
}
#[doc(hidden)]
#[allow(non_snake_case)]
pub unsafe fn __post_return_search_file<T: Guest>(arg0: *mut u8) {
    let l0 = i32::from(*arg0.add(0).cast::<u8>());
    match l0 {
        0 => {
            let l1 = *arg0.add(4).cast::<*mut u8>();
            let l2 = *arg0.add(8).cast::<usize>();
            _rt::cabi_dealloc(l1, l2, 1);
        }
        _ => {
            let l3 = *arg0.add(4).cast::<*mut u8>();
            let l4 = *arg0.add(8).cast::<usize>();
            _rt::cabi_dealloc(l3, l4, 1);
        }
    }
}
#[doc(hidden)]
#[allow(non_snake_case)]
pub unsafe fn _export_get_file_info_cabi<T: Guest>(
    arg0: *mut u8,
    arg1: usize,
) -> *mut u8 {
    #[cfg(target_arch = "wasm32")] _rt::run_ctors_once();
    let len0 = arg1;
    let bytes0 = _rt::Vec::from_raw_parts(arg0.cast(), len0, len0);
    let result1 = T::get_file_info(_rt::string_lift(bytes0));
    let ptr2 = _RET_AREA.0.as_mut_ptr().cast::<u8>();
    match result1 {
        Ok(e) => {
            *ptr2.add(0).cast::<u8>() = (0i32) as u8;
            let vec3 = (e.into_bytes()).into_boxed_slice();
            let ptr3 = vec3.as_ptr().cast::<u8>();
            let len3 = vec3.len();
            ::core::mem::forget(vec3);
            *ptr2.add(8).cast::<usize>() = len3;
            *ptr2.add(4).cast::<*mut u8>() = ptr3.cast_mut();
        }
        Err(e) => {
            *ptr2.add(0).cast::<u8>() = (1i32) as u8;
            let vec4 = (e.into_bytes()).into_boxed_slice();
            let ptr4 = vec4.as_ptr().cast::<u8>();
            let len4 = vec4.len();
            ::core::mem::forget(vec4);
            *ptr2.add(8).cast::<usize>() = len4;
            *ptr2.add(4).cast::<*mut u8>() = ptr4.cast_mut();
        }
    };
    ptr2
}
#[doc(hidden)]
#[allow(non_snake_case)]
pub unsafe fn __post_return_get_file_info<T: Guest>(arg0: *mut u8) {
    let l0 = i32::from(*arg0.add(0).cast::<u8>());
    match l0 {
        0 => {
            let l1 = *arg0.add(4).cast::<*mut u8>();
            let l2 = *arg0.add(8).cast::<usize>();
            _rt::cabi_dealloc(l1, l2, 1);
        }
        _ => {
            let l3 = *arg0.add(4).cast::<*mut u8>();
            let l4 = *arg0.add(8).cast::<usize>();
            _rt::cabi_dealloc(l3, l4, 1);
        }
    }
}
pub trait Guest {
    /// Get a detailed listing of all files and directories in a specified path.
    /// Results clearly distinguish between files and directories with [FILE] and [DIR] \
    /// prefixes. This tool is essential for understanding directory structure and \
    /// finding specific files within a directory. Only works within allowed directories.
    fn list_directory(path: _rt::String) -> Result<_rt::Vec<_rt::String>, _rt::String>;
    /// Read the complete contents of a file from the file system.
    fn read_file(path: _rt::String) -> Result<_rt::String, _rt::String>;
    /// Recursively search for files and directories matching a pattern.
    fn search_file(
        path: _rt::String,
        pattern: _rt::String,
    ) -> Result<_rt::String, _rt::String>;
    /// Retrieve detailed metadata about a file or directory.
    fn get_file_info(path: _rt::String) -> Result<_rt::String, _rt::String>;
}
#[doc(hidden)]
macro_rules! __export_world_fs_cabi {
    ($ty:ident with_types_in $($path_to_types:tt)*) => {
        const _ : () = { #[export_name = "list-directory"] unsafe extern "C" fn
        export_list_directory(arg0 : * mut u8, arg1 : usize,) -> * mut u8 {
        $($path_to_types)*:: _export_list_directory_cabi::<$ty > (arg0, arg1) }
        #[export_name = "cabi_post_list-directory"] unsafe extern "C" fn
        _post_return_list_directory(arg0 : * mut u8,) { $($path_to_types)*::
        __post_return_list_directory::<$ty > (arg0) } #[export_name = "read-file"] unsafe
        extern "C" fn export_read_file(arg0 : * mut u8, arg1 : usize,) -> * mut u8 {
        $($path_to_types)*:: _export_read_file_cabi::<$ty > (arg0, arg1) } #[export_name
        = "cabi_post_read-file"] unsafe extern "C" fn _post_return_read_file(arg0 : * mut
        u8,) { $($path_to_types)*:: __post_return_read_file::<$ty > (arg0) }
        #[export_name = "search-file"] unsafe extern "C" fn export_search_file(arg0 : *
        mut u8, arg1 : usize, arg2 : * mut u8, arg3 : usize,) -> * mut u8 {
        $($path_to_types)*:: _export_search_file_cabi::<$ty > (arg0, arg1, arg2, arg3) }
        #[export_name = "cabi_post_search-file"] unsafe extern "C" fn
        _post_return_search_file(arg0 : * mut u8,) { $($path_to_types)*::
        __post_return_search_file::<$ty > (arg0) } #[export_name = "get-file-info"]
        unsafe extern "C" fn export_get_file_info(arg0 : * mut u8, arg1 : usize,) -> *
        mut u8 { $($path_to_types)*:: _export_get_file_info_cabi::<$ty > (arg0, arg1) }
        #[export_name = "cabi_post_get-file-info"] unsafe extern "C" fn
        _post_return_get_file_info(arg0 : * mut u8,) { $($path_to_types)*::
        __post_return_get_file_info::<$ty > (arg0) } };
    };
}
#[doc(hidden)]
pub(crate) use __export_world_fs_cabi;
#[repr(align(4))]
struct _RetArea([::core::mem::MaybeUninit<u8>; 12]);
static mut _RET_AREA: _RetArea = _RetArea([::core::mem::MaybeUninit::uninit(); 12]);
mod _rt {
    #[cfg(target_arch = "wasm32")]
    pub fn run_ctors_once() {
        wit_bindgen_rt::run_ctors_once();
    }
    pub use alloc_crate::vec::Vec;
    pub unsafe fn string_lift(bytes: Vec<u8>) -> String {
        if cfg!(debug_assertions) {
            String::from_utf8(bytes).unwrap()
        } else {
            String::from_utf8_unchecked(bytes)
        }
    }
    pub use alloc_crate::alloc;
    pub unsafe fn cabi_dealloc(ptr: *mut u8, size: usize, align: usize) {
        if size == 0 {
            return;
        }
        let layout = alloc::Layout::from_size_align_unchecked(size, align);
        alloc::dealloc(ptr, layout);
    }
    pub use alloc_crate::string::String;
    extern crate alloc as alloc_crate;
}
/// Generates `#[no_mangle]` functions to export the specified type as the
/// root implementation of all generated traits.
///
/// For more information see the documentation of `wit_bindgen::generate!`.
///
/// ```rust
/// # macro_rules! export{ ($($t:tt)*) => (); }
/// # trait Guest {}
/// struct MyType;
///
/// impl Guest for MyType {
///     // ...
/// }
///
/// export!(MyType);
/// ```
#[allow(unused_macros)]
#[doc(hidden)]
macro_rules! __export_fs_impl {
    ($ty:ident) => {
        self::export!($ty with_types_in self);
    };
    ($ty:ident with_types_in $($path_to_types_root:tt)*) => {
        $($path_to_types_root)*:: __export_world_fs_cabi!($ty with_types_in
        $($path_to_types_root)*);
    };
}
#[doc(inline)]
pub(crate) use __export_fs_impl as export;
#[cfg(target_arch = "wasm32")]
#[link_section = "component-type:wit-bindgen:0.30.0:fs:encoded world"]
#[doc(hidden)]
pub static __WIT_BINDGEN_COMPONENT_TYPE: [u8; 280] = *b"\
\0asm\x0d\0\x01\0\0\x19\x16wit-component-encoding\x04\0\x07\x9f\x01\x01A\x02\x01\
A\x0a\x01ps\x01j\x01\0\x01s\x01@\x01\x04paths\0\x01\x04\0\x0elist-directory\x01\x02\
\x01j\x01s\x01s\x01@\x01\x04paths\0\x03\x04\0\x09read-file\x01\x04\x01@\x02\x04p\
aths\x07patterns\0\x03\x04\0\x0bsearch-file\x01\x05\x04\0\x0dget-file-info\x01\x04\
\x04\x01\x18component:filesystem2/fs\x04\0\x0b\x08\x01\0\x02fs\x03\0\0\0G\x09pro\
ducers\x01\x0cprocessed-by\x02\x0dwit-component\x070.215.0\x10wit-bindgen-rust\x06\
0.30.0";
#[inline(never)]
#[doc(hidden)]
pub fn __link_custom_section_describing_imports() {
    wit_bindgen_rt::maybe_link_cabi_realloc();
}
