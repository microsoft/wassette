// Generated bindings for arxiv component

#[doc(hidden)]
#[allow(non_snake_case)]
pub unsafe fn _export_search_papers_cabi<T: Guest>(
    arg0: *mut u8,
    arg1: usize,
    arg2: u32,
    arg3: *mut u8,
    arg4: usize,
    arg5: *mut u8,
    arg6: usize,
) -> *mut u8 {
    #[cfg(target_arch = "wasm32")]
    _rt::run_ctors_once();
    let len0 = arg1;
    let bytes0 = _rt::Vec::from_raw_parts(arg0.cast(), len0, len0);
    let len1 = arg4;
    let bytes1 = _rt::Vec::from_raw_parts(arg3.cast(), len1, len1);
    let len2 = arg6;
    let bytes2 = _rt::Vec::from_raw_parts(arg5.cast(), len2, len2);
    let result3 = T::search_papers(
        _rt::string_lift(bytes0),
        arg2,
        _rt::string_lift(bytes1),
        _rt::string_lift(bytes2),
    );
    let ptr4 = _RET_AREA.0.as_mut_ptr().cast::<u8>();
    match result3 {
        Ok(e) => {
            *ptr4.add(0).cast::<u8>() = (0i32) as u8;
            let vec5 = (e.into_bytes()).into_boxed_slice();
            let ptr5 = vec5.as_ptr().cast::<u8>();
            let len5 = vec5.len();
            ::core::mem::forget(vec5);
            *ptr4.add(8).cast::<usize>() = len5;
            *ptr4.add(4).cast::<*mut u8>() = ptr5.cast_mut();
        }
        Err(e) => {
            *ptr4.add(0).cast::<u8>() = (1i32) as u8;
            let vec6 = (e.into_bytes()).into_boxed_slice();
            let ptr6 = vec6.as_ptr().cast::<u8>();
            let len6 = vec6.len();
            ::core::mem::forget(vec6);
            *ptr4.add(8).cast::<usize>() = len6;
            *ptr4.add(4).cast::<*mut u8>() = ptr6.cast_mut();
        }
    };
    ptr4
}

#[doc(hidden)]
#[allow(non_snake_case)]
pub unsafe fn __post_return_search_papers<T: Guest>(arg0: *mut u8) {
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
pub unsafe fn _export_download_paper_cabi<T: Guest>(arg0: *mut u8, arg1: usize) -> *mut u8 {
    #[cfg(target_arch = "wasm32")]
    _rt::run_ctors_once();
    let len0 = arg1;
    let bytes0 = _rt::Vec::from_raw_parts(arg0.cast(), len0, len0);
    let result1 = T::download_paper(_rt::string_lift(bytes0));
    let ptr2 = _RET_AREA.0.as_mut_ptr().cast::<u8>();
    match result1 {
        Ok(e) => {
            *ptr2.add(0).cast::<u8>() = (0i32) as u8;
            let vec3 = e.into_boxed_slice();
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
pub unsafe fn __post_return_download_paper<T: Guest>(arg0: *mut u8) {
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
pub unsafe fn _export_read_paper_cabi<T: Guest>(arg0: *mut u8, arg1: usize) -> *mut u8 {
    #[cfg(target_arch = "wasm32")]
    _rt::run_ctors_once();
    let len0 = arg1;
    let bytes0 = _rt::Vec::from_raw_parts(arg0.cast(), len0, len0);
    let result1 = T::read_paper(_rt::string_lift(bytes0));
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
pub unsafe fn __post_return_read_paper<T: Guest>(arg0: *mut u8) {
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
    fn search_papers(
        query: _rt::String,
        max_results: u32,
        date_from: _rt::String,
        categories: _rt::String,
    ) -> Result<_rt::String, _rt::String>;
    fn download_paper(id: _rt::String) -> Result<_rt::Vec<u8>, _rt::String>;
    fn read_paper(id: _rt::String) -> Result<_rt::String, _rt::String>;
}

#[doc(hidden)]
macro_rules! __export_world_arxiv_cabi {
    ($ty:ident with_types_in $($path_to_types:tt)*) => {
        const _ : () = {
            #[export_name = "search-papers"]
            unsafe extern "C" fn export_search_papers(
                arg0: *mut u8,
                arg1: usize,
                arg2: u32,
                arg3: *mut u8,
                arg4: usize,
                arg5: *mut u8,
                arg6: usize,
            ) -> *mut u8 {
                $($path_to_types)*:: _export_search_papers_cabi::<$ty>(arg0, arg1, arg2, arg3, arg4, arg5, arg6)
            }
            #[export_name = "cabi_post_search-papers"]
            unsafe extern "C" fn _post_return_search_papers(arg0: *mut u8) {
                $($path_to_types)*:: __post_return_search_papers::<$ty>(arg0)
            }

            #[export_name = "download-paper"]
            unsafe extern "C" fn export_download_paper(arg0: *mut u8, arg1: usize) -> *mut u8 {
                $($path_to_types)*:: _export_download_paper_cabi::<$ty>(arg0, arg1)
            }
            #[export_name = "cabi_post_download-paper"]
            unsafe extern "C" fn _post_return_download_paper(arg0: *mut u8) {
                $($path_to_types)*:: __post_return_download_paper::<$ty>(arg0)
            }

            #[export_name = "read-paper"]
            unsafe extern "C" fn export_read_paper(arg0: *mut u8, arg1: usize) -> *mut u8 {
                $($path_to_types)*:: _export_read_paper_cabi::<$ty>(arg0, arg1)
            }
            #[export_name = "cabi_post_read-paper"]
            unsafe extern "C" fn _post_return_read_paper(arg0: *mut u8) {
                $($path_to_types)*:: __post_return_read_paper::<$ty>(arg0)
            }
        };
    };
}

#[doc(hidden)]
pub(crate) use __export_world_arxiv_cabi;

#[repr(align(4))]
struct _RetArea([::core::mem::MaybeUninit<u8>; 12]);
static mut _RET_AREA: _RetArea = _RetArea([::core::mem::MaybeUninit::uninit(); 12]);

mod _rt {
    #[cfg(target_arch = "wasm32")]
    pub fn run_ctors_once() {
        wit_bindgen_rt::run_ctors_once();
    }

    pub use alloc_crate::string::String;
    pub use alloc_crate::vec::Vec;

    pub unsafe fn string_lift(bytes: Vec<u8>) -> String {
        if cfg!(debug_assertions) {
            String::from_utf8(bytes).unwrap()
        } else {
            String::from_utf8_unchecked(bytes)
        }
    }

    pub unsafe fn cabi_dealloc(ptr: *mut u8, size: usize, align: usize) {
        if size == 0 {
            return;
        }
        let layout = alloc::Layout::from_size_align_unchecked(size, align);
        alloc::dealloc(ptr, layout);
    }

    extern crate alloc as alloc_crate;
    pub use alloc_crate::alloc;
}

/// Generates `#[no_mangle]` functions to export the specified type as the
/// root implementation of all generated traits.
#[allow(unused_macros)]
#[doc(hidden)]
macro_rules! __export_arxiv_impl {
    ($ty:ident) => {
        self::export!($ty with_types_in self);
    };
    ($ty:ident with_types_in $($path_to_types_root:tt)*) => {
        $($path_to_types_root)*:: __export_world_arxiv_cabi!($ty with_types_in
        $($path_to_types_root)*);
    };
}

#[doc(inline)]
pub(crate) use __export_arxiv_impl as export;
