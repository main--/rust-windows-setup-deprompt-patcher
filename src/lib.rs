#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

extern crate memmap;
extern crate memmem;

include!(concat!(env!("OUT_DIR"), "/bindings.rs"));

const UDF_BLOCKSIZE: usize = udf_enum1_t::UDF_BLOCKSIZE as usize;

use std::fs::OpenOptions;
use std::ffi::{CStr, CString};
use std::path::Path;
use std::marker::PhantomData;
use std::io::Error as IoError;

use memmap::MmapMut;
use memmem::{Searcher, TwoWaySearcher};

/// No error information? FeelsBadMan
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UdfError;

pub struct Udf(*mut udf_t);

impl Udf {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Udf, UdfError> {
        let cstr = CString::new(path.as_ref().to_str().unwrap()).unwrap();
        let udf = unsafe { udf_open(cstr.as_ptr()) };
        if udf.is_null() {
            Err(UdfError)
        } else {
            Ok(Udf(udf))
        }
    }

    pub fn root_directory(&self, partition: Option<u16>) -> Result<UdfDirent, UdfError> {
        let dirent = unsafe { udf_get_root(self.0, partition.is_none() as u8, partition.unwrap_or(0)) };
        if dirent.is_null() {
            Err(UdfError)
        } else {
            Ok(UdfDirent { ptr: dirent, udf: PhantomData })
        }
    }
}

impl Drop for Udf {
    fn drop(&mut self) {
        unsafe {
            assert!(udf_close(self.0) > 0);
        }
    }
}

pub struct UdfDirent<'udf> {
    ptr: *mut udf_dirent_t,
    udf: PhantomData<&'udf Udf>,
}

impl<'a> UdfDirent<'a> {
    pub fn filename(&self) -> &CStr {
        unsafe { CStr::from_ptr(udf_get_filename(self.ptr)) }
    }

    pub fn length(&self) -> Result<u64, UdfError> {
        let len = unsafe { udf_get_file_length(self.ptr) };
        if len == 0x7fffffff {
            Err(UdfError)
        } else {
            Ok(len)
        }
    }

    pub fn open_file(&self, name: &str) -> Result<UdfDirent, UdfError> {
        let name = CString::new(name).unwrap();
        let dirent = unsafe { udf_fopen(self.ptr, name.as_ptr()) };
        if dirent.is_null() {
            Err(UdfError)
        } else {
            Ok(UdfDirent { ptr: dirent, udf: PhantomData })
        }
    }

    pub fn read(&self) -> Result<Vec<u8>, UdfError> {
        // evil hack to reset the global (wtf!) file cursor
        unsafe {
            assert!(udf_fopen(self.ptr, b"\0".as_ptr() as *const _).is_null());
        }

        let len = self.length()? as usize;
        let blocks = (len + UDF_BLOCKSIZE - 1) / UDF_BLOCKSIZE;
        let mut buffer = Vec::with_capacity(blocks * UDF_BLOCKSIZE);
        let ret = unsafe { udf_read_block(self.ptr, buffer.as_mut_ptr() as *mut _, blocks) };
        if ret == len as isize {
            unsafe { buffer.set_len(len) };
            Ok(buffer)
        } else {
            Err(UdfError)
        }
    }
}

impl<'a> Drop for UdfDirent<'a> {
    fn drop(&mut self) {
        unsafe {
            assert!(udf_dirent_free(self.ptr) > 0);
        }
    }
}


fn grab_blobs(path: &str) -> Result<(Vec<u8>, Vec<u8>), UdfError> {
    let udf = Udf::open(path)?;
    let root = udf.root_directory(None)?;
    let cdboot = root.open_file("/efi/microsoft/boot/cdboot.efi")?.read()?;
    let cdboot_noprompt = root.open_file("/efi/microsoft/boot/cdboot_noprompt.efi")?.read()?;
    Ok((cdboot, cdboot_noprompt))
}

#[derive(Debug)]
pub enum PatchError {
    Udf(UdfError),
    Io(IoError),
    InvalidIsoFormat,
}

pub fn patch(path: &str, want_prompt: bool) -> Result<bool, PatchError> {
    let (cdboot, cdboot_noprompt) = grab_blobs(path).map_err(PatchError::Udf)?;
    if cdboot.len() != cdboot_noprompt.len() {
        return Err(PatchError::InvalidIsoFormat);
    }

    let (from, to) = if want_prompt {
        (cdboot_noprompt, cdboot)
    } else {
        (cdboot, cdboot_noprompt)
    };

    let iso = OpenOptions::new().read(true).write(true).create(false)
        .append(false).truncate(false).open(path).map_err(PatchError::Io)?;
    let mut map = unsafe { MmapMut::map_mut(&iso).map_err(PatchError::Io)? };

    // we only search the first megabyte because it has to be at the start
    let index = match TwoWaySearcher::new(&from).search_in(&map[..0x1_000_000]) {
        Some(i) => i,
        None => {
            let patch_found = TwoWaySearcher::new(&to).search_in(&map[..0x1_000_000]).is_some();
            if patch_found {
                return Ok(false);
            } else {
                return Err(PatchError::InvalidIsoFormat);
            }
        }
    };

    map[index..][..from.len()].copy_from_slice(&to);
    map.flush().map_err(PatchError::Io)?;
    Ok(true)
}
