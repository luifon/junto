//! The status item of the application in the menu bar of macOS, with its menu (ADR 0759). A
//! menu entry calls an Objective-C method of a target object; the class of that object is
//! defined here, which needs unsafe code.

#![allow(unsafe_code)]

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{
    AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel,
};
use objc2_app_kit::{
    NSImage, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem, NSVariableStatusItemLength,
};
use objc2_foundation::{NSData, NSSize, NSString};

/// One entry of the menu.
pub struct Entry<'a> {
    pub label: &'a str,
    /// The number that a press of the entry gives to the callback of the item.
    pub code: u32,
    pub enabled: bool,
    /// A separator line comes before the entry.
    pub separated: bool,
}

struct Ivars {
    /// Called with the code of the pressed entry.
    pressed: Box<dyn Fn(u32)>,
}

define_class!(
    // SAFETY: NSObject has no requirement for a subclass, and `Target` does not implement
    // `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "JuntoMenuBarTarget"]
    #[ivars = Ivars]
    struct Target;

    impl Target {
        // SAFETY: the signature matches an action of a menu entry: one argument, the entry.
        #[unsafe(method(pressed:))]
        fn pressed(&self, sender: &NSMenuItem) {
            if let Ok(code) = u32::try_from(sender.tag()) {
                (self.ivars().pressed)(code);
            }
        }
    }
);

impl Target {
    fn new(mtm: MainThreadMarker, pressed: Box<dyn Fn(u32)>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars { pressed });
        // SAFETY: `init` of NSObject on an allocated object whose instance variables are set.
        unsafe { msg_send![super(this), init] }
    }
}

/// A status item in the menu bar. It is removed from the menu bar when it is dropped.
pub struct StatusItem {
    item: Retained<NSStatusItem>,
    /// A menu entry holds its target without keeping it: the item keeps it.
    target: Retained<Target>,
    mtm: MainThreadMarker,
}

impl StatusItem {
    /// A new item at the end of the status items. Absent when the caller is not on the main
    /// thread, where AppKit runs.
    pub fn new(pressed: Box<dyn Fn(u32)>) -> Option<Self> {
        let mtm = MainThreadMarker::new()?;
        let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        Some(Self {
            item,
            target: Target::new(mtm, pressed),
            mtm,
        })
    }

    /// Shows the PNG picture at `points` by `points` points, the text that the menu bar shows
    /// when the pointer rests on the item, and the menu with these entries.
    pub fn set(&self, png: &[u8], points: f64, tooltip: &str, entries: &[Entry<'_>]) {
        if let Some(button) = self.item.button(self.mtm) {
            let data = NSData::with_bytes(png);
            match NSImage::initWithData(NSImage::alloc(), &data) {
                Some(image) => {
                    image.setSize(NSSize::new(points, points));
                    button.setImage(Some(&image));
                }
                None => tracing::warn!("the picture of the menu bar item was not read"),
            }
            button.setToolTip(Some(&NSString::from_str(tooltip)));
        }
        let menu = NSMenu::new(self.mtm);
        // The entries are enabled as `entries` states, not by the responder chain.
        menu.setAutoenablesItems(false);
        let target: &AnyObject = &self.target;
        let no_key = NSString::from_str("");
        for entry in entries {
            if entry.separated {
                menu.addItem(&NSMenuItem::separatorItem(self.mtm));
            }
            // SAFETY: the action `pressed:` takes the menu entry as its one argument, and its
            // target lives as long as this item, which owns the menu.
            let item = unsafe {
                let item = NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(self.mtm),
                    &NSString::from_str(entry.label),
                    Some(sel!(pressed:)),
                    &no_key,
                );
                item.setTarget(Some(target));
                item
            };
            item.setTag(entry.code as isize);
            item.setEnabled(entry.enabled);
            menu.addItem(&item);
        }
        self.item.setMenu(Some(&menu));
    }
}

impl Drop for StatusItem {
    fn drop(&mut self) {
        NSStatusBar::systemStatusBar().removeStatusItem(&self.item);
    }
}
