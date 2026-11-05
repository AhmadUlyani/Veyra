//! File shelf: OLE drop target for dragging files in, shell drag source for
//! dragging them back out.

use std::sync::atomic::{AtomicBool, Ordering};

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Ole::*;
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::*;

use crate::events::{post, Event};
use crate::util::wide;

static DRAGGING_OUT: AtomicBool = AtomicBool::new(false);
static DRAG_OK: AtomicBool = AtomicBool::new(false);

fn hdrop_fmt() -> FORMATETC {
    FORMATETC {
        cfFormat: CF_HDROP.0,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    }
}

fn get_files(d: &IDataObject) -> Vec<String> {
    let mut out = Vec::new();
    unsafe {
        let Ok(mut med) = d.GetData(&hdrop_fmt()) else { return out };
        let hdrop = HDROP(med.u.hGlobal.0);
        let n = DragQueryFileW(hdrop, u32::MAX, None);
        for i in 0..n {
            let len = DragQueryFileW(hdrop, i, None) as usize;
            let mut buf = vec![0u16; len + 1];
            DragQueryFileW(hdrop, i, Some(&mut buf));
            out.push(String::from_utf16_lossy(&buf[..len]));
        }
        ReleaseStgMedium(&mut med);
    }
    out
}

#[implement(IDropTarget)]
pub struct DropTarget;

impl IDropTarget_Impl for DropTarget_Impl {
    fn DragEnter(
        &self,
        pdataobj: Option<&IDataObject>,
        _ks: MODIFIERKEYS_FLAGS,
        _pt: &POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> Result<()> {
        let ok = !DRAGGING_OUT.load(Ordering::Relaxed) && pdataobj.map(|d| !get_files(d).is_empty()).unwrap_or(false);
        DRAG_OK.store(ok, Ordering::Relaxed);
        unsafe {
            if !pdweffect.is_null() {
                *pdweffect = if ok { DROPEFFECT_COPY } else { DROPEFFECT_NONE };
            }
        }
        if ok {
            post(Event::DragEnter);
        }
        Ok(())
    }

    fn DragOver(&self, _ks: MODIFIERKEYS_FLAGS, _pt: &POINTL, pdweffect: *mut DROPEFFECT) -> Result<()> {
        unsafe {
            if !pdweffect.is_null() {
                *pdweffect = if DRAG_OK.load(Ordering::Relaxed) { DROPEFFECT_COPY } else { DROPEFFECT_NONE };
            }
        }
        Ok(())
    }

    fn DragLeave(&self) -> Result<()> {
        if DRAG_OK.swap(false, Ordering::Relaxed) {
            post(Event::DragLeave);
        }
        Ok(())
    }

    fn Drop(
        &self,
        pdataobj: Option<&IDataObject>,
        _ks: MODIFIERKEYS_FLAGS,
        _pt: &POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> Result<()> {
        let ok = DRAG_OK.swap(false, Ordering::Relaxed);
        let files = if ok { pdataobj.map(get_files).unwrap_or_default() } else { Vec::new() };
        unsafe {
            if !pdweffect.is_null() {
                *pdweffect = if files.is_empty() { DROPEFFECT_NONE } else { DROPEFFECT_COPY };
            }
        }
        post(Event::Drop(files));
        Ok(())
    }
}

pub fn register(hwnd: HWND) {
    unsafe {
        let t: IDropTarget = DropTarget.into();
        let _ = RegisterDragDrop(hwnd, &t);
    }
}

/// Build a shell data object carrying all the given files (verified to work for files
/// from different folders).
pub fn data_object(paths: &[String]) -> Option<IDataObject> {
    unsafe {
        let mut pidls: Vec<*const ITEMIDLIST> = Vec::new();
        for p in paths {
            let w = wide(p);
            let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
            if SHParseDisplayName(PCWSTR(w.as_ptr()), None, &mut pidl, 0, None).is_ok() && !pidl.is_null() {
                pidls.push(pidl as *const _);
            }
        }
        let obj = if pidls.is_empty() {
            None
        } else {
            SHCreateShellItemArrayFromIDLists(&pidls)
                .ok()
                .and_then(|arr| arr.BindToHandler::<_, IDataObject>(None, &BHID_DataObject).ok())
        };
        for p in pidls {
            CoTaskMemFree(Some(p as *const _));
        }
        obj
    }
}

/// Start a shell drag of the given files (blocks until the drop completes).
pub fn drag_out(hwnd: HWND, paths: &[String]) {
    let Some(obj) = data_object(paths) else { return };
    DRAGGING_OUT.store(true, Ordering::Relaxed);
    unsafe {
        let _ = SHDoDragDrop(hwnd, &obj, None, DROPEFFECT_COPY | DROPEFFECT_LINK);
    }
    DRAGGING_OUT.store(false, Ordering::Relaxed);
}

pub fn open(path: &str) {
    let w = wide(path);
    unsafe {
        ShellExecuteW(HWND::default(), w!("open"), PCWSTR(w.as_ptr()), PCWSTR::null(), PCWSTR::null(), windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL);
    }
}
