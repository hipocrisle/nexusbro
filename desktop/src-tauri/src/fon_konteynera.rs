//! Незаполненный CEF участок разделителя рисуется цветом темы, без белого STATIC.
#[cfg(target_os="windows")]
mod win{
 use std::{cell::RefCell,collections::HashMap};
 use windows::Win32::{Foundation::*,Graphics::Gdi::*,UI::WindowsAndMessaging::*};
 thread_local!{static WINDOWS:RefCell<HashMap<isize,(u32,isize)>>=RefCell::new(HashMap::new());}
 unsafe extern "system" fn proc(hwnd:HWND,msg:u32,w:WPARAM,l:LPARAM)->LRESULT{
  let state=WINDOWS.with(|m|m.borrow().get(&(hwnd.0 as isize)).copied());
  if msg==WM_ERASEBKGND{return LRESULT(1);}
  if msg==WM_PAINT{
   let mut ps=PAINTSTRUCT::default();let dc=BeginPaint(hwnd,&mut ps);
   let rgb=state.map(|s|s.0).unwrap_or(0x171a24);
   let brush=CreateSolidBrush(COLORREF(((rgb&255)<<16)|(rgb&0xff00)|((rgb>>16)&255)));
   let _=FillRect(dc,&ps.rcPaint,brush);let _=DeleteObject(brush);let _=EndPaint(hwnd,&ps);
   return LRESULT(0);
  }
  if msg==WM_NCDESTROY{WINDOWS.with(|m|{m.borrow_mut().remove(&(hwnd.0 as isize));});}
  if let Some((_,previous))=state{CallWindowProcW(std::mem::transmute::<isize,WNDPROC>(previous),hwnd,msg,w,l)}else{DefWindowProcW(hwnd,msg,w,l)}
 }
 pub fn set(hwnd:u64,rgb:u32){unsafe{
  let h=HWND(hwnd as usize as _);
  WINDOWS.with(|m|{let mut m=m.borrow_mut();
   if let Some(value)=m.get_mut(&(h.0 as isize)){if value.0==rgb{return;}value.0=rgb;}
   else{let previous=SetWindowLongPtrW(h,GWLP_WNDPROC,proc as *const () as isize);if previous==0{return;}m.insert(h.0 as isize,(rgb,previous));}
   let _=InvalidateRect(h,None,false);
  });
 }}
}
#[cfg(target_os="windows")]pub use win::set;
#[cfg(not(target_os="windows"))]pub fn set(_:u64,_:u32){}
