//! The scroll position of the open page of Server settings, which the screen tool and the
//! window tests set and read.

use crate::shell::Shell;

impl Shell {
    /// Shows the end of the open page of the administration. The screen tool uses it.
    #[cfg(feature = "test-support")]
    pub fn scroll_admin_page_to_end(&mut self) {
        self.admin.list.requested_end = true;
        self.admin.list.state.scroll_to_end();
        self.admin.page_scroll.scroll_to_bottom();
    }

    #[cfg(feature = "test-support")]
    pub fn scroll_admin_page_by(&mut self, distance: gpui::Pixels) {
        if self.admin.list.state.item_count() > 0 {
            self.admin.list.state.scroll_by(distance);
        } else {
            let mut offset = self.admin.page_scroll.offset();
            offset.y -= distance;
            self.admin.page_scroll.set_offset(offset);
        }
    }

    /// Moves the open page of the administration to a place between its start (0) and its
    /// end (1), as a drag of the scroll bar does. The screen tool uses it.
    #[cfg(feature = "test-support")]
    pub fn scroll_admin_page_to(&mut self, fraction: f32) {
        if self.admin.list.state.item_count() > 0 {
            let state = &self.admin.list.state;
            let end = state.max_offset_for_scrollbar().y;
            state.set_offset_from_scrollbar(gpui::point(gpui::px(0.0), -end * fraction));
        } else {
            let end = self.admin.page_scroll.max_offset().y;
            self.admin
                .page_scroll
                .set_offset(gpui::point(gpui::px(0.0), -end * fraction));
        }
    }

    /// The height of the content of the open page of the administration, as its scroll
    /// bar takes it.
    #[cfg(feature = "test-support")]
    pub fn admin_page_extent(&self) -> gpui::Pixels {
        let state = &self.admin.list.state;
        state.max_offset_for_scrollbar().y + state.viewport_bounds().size.height
    }

    #[cfg(feature = "test-support")]
    pub fn admin_page_at_end(&self) -> bool {
        self.admin.list.state.is_scrolled_to_end() == Some(true)
    }

    #[cfg(feature = "test-support")]
    pub fn scroll_admin_page_to_start(&mut self) {
        self.admin.list.state.scroll_to(gpui::ListOffset::default());
        self.admin
            .page_scroll
            .set_offset(gpui::point(gpui::px(0.0), gpui::px(0.0)));
    }
}
