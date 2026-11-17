//! Разделитель нативных CEF-панелей: указатель читается в потоке их UI.
use std::{cell::RefCell,collections::HashMap};
#[derive(Default)]struct Drag{down:bool,active:bool,hover:bool,grab:i32}
thread_local!{static DRAGS:RefCell<HashMap<u64,Drag>>=RefCell::new(HashMap::new());}
pub fn shag(hwnd:u64,w:u32,h:u32,panel:u32)->Option<u32>{
 let (x,y,down,own)=pointer(hwnd)?;
 let panel=panel.max(280).min(w.saturating_sub(280));
 let border=w.saturating_sub(panel+6) as i32;
 let over=own&&x>=border&&x<border+6&&y>=0&&y<h as i32;
 DRAGS.with(|m|{let mut m=m.borrow_mut();let d=m.entry(hwnd).or_default();
  if down&&!d.down&&over{d.active=true;d.grab=x-border;capture(hwnd,true);}
  if !down&&d.active{d.active=false;capture(hwnd,false);}
  d.down=down;let hover=over||d.active;if hover!=d.hover||(cfg!(target_os="windows")&&hover){cursor(hwnd,hover);d.hover=hover;}
  if d.active&&w>560{Some(((w as i32-x+d.grab-6).max(280) as u32).min(w-280))}else{None}
 })
}
#[cfg(target_os="windows")]
fn pointer(hwnd:u64)->Option<(i32,i32,bool,bool)>{use windows::Win32::{Foundation::*,UI::{WindowsAndMessaging::*,Input::KeyboardAndMouse::*}};unsafe{let mut p=POINT::default();GetCursorPos(&mut p).ok()?;let target=HWND(hwnd as usize as *mut _);let hit=WindowFromPoint(p);/* STATIC без SS_NOTIFY пропускает hit-test к родителю. */let own=IsWindowVisible(target).as_bool()&&(hit==target||GetParent(target).ok()==Some(hit));windows::Win32::Graphics::Gdi::ScreenToClient(HWND(hwnd as usize as *mut _),&mut p).ok().ok()?;Some((p.x,p.y,GetAsyncKeyState(VK_LBUTTON.0 as i32)<0,own))}}
#[cfg(target_os="windows")]
fn capture(hwnd:u64,on:bool){use windows::Win32::{Foundation::HWND,UI::Input::KeyboardAndMouse::*};unsafe{if on{SetCapture(HWND(hwnd as usize as *mut _));}else{let _=ReleaseCapture();}}}
#[cfg(target_os="windows")]
fn cursor(_:u64,on:bool){use windows::Win32::UI::WindowsAndMessaging::*;unsafe{if let Ok(c)=LoadCursorW(None,if on{IDC_SIZEWE}else{IDC_ARROW}){SetCursor(c);}}}
#[cfg(target_os="linux")]
thread_local!{static X:Option<(x11rb::rust_connection::RustConnection,u32)>= (||{use x11rb::{connection::Connection,protocol::xproto::*};let(c,_)=x11rb::connect(None).ok()?;let f=c.generate_id().ok()?;let cursor=c.generate_id().ok()?;c.open_font(f,b"cursor").ok()?.check().ok()?;c.create_glyph_cursor(cursor,f,f,108,109,0,0,0,65535,65535,65535).ok()?.check().ok()?;let _=c.close_font(f);Some((c,cursor))})();}
#[cfg(target_os="linux")]
fn pointer(hwnd:u64)->Option<(i32,i32,bool,bool)>{use x11rb::protocol::xproto::*;X.with(|x|{let(c,_)=x.as_ref()?;let p=c.query_pointer(hwnd as u32).ok()?.reply().ok()?;Some((p.win_x as i32,p.win_y as i32,p.mask.contains(KeyButMask::BUTTON1),p.child==0&&p.same_screen))})}
#[cfg(target_os="linux")]
fn cursor(hwnd:u64,on:bool){use x11rb::{connection::Connection,protocol::xproto::*};X.with(|x|{if let Some((c,cursor))=x{let _=c.change_window_attributes(hwnd as u32,&ChangeWindowAttributesAux::new().cursor(if on{*cursor}else{0}));let _=c.flush();}});}
#[cfg(not(target_os="windows"))]fn capture(_:u64,_:bool){}
#[cfg(not(any(target_os="windows",target_os="linux")))]fn cursor(_:u64,_:bool){}
#[cfg(not(any(target_os="windows",target_os="linux")))]fn pointer(_:u64)->Option<(i32,i32,bool,bool)>{None}
