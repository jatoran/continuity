//! Timeline, tutorial, and history-tab `ViewContext` methods.

macro_rules! view_context_timeline_methods {
    () => {
        /// Open the previous-buffer browser overlay. Lists persisted
        /// buffers, including closed tabs and excluding trash by default.
        fn show_previous_buffer_browser(&mut self) -> Result<(), Error> {
            Err(Error::UnsupportedContext("show_previous_buffer_browser"))
        }

        /// Open the time-machine slider against a specific closed buffer.
        fn open_timeline_for_closed_buffer(
            &mut self,
            _buffer_id: continuity_buffer::BufferId,
        ) -> Result<(), Error> {
            Err(Error::UnsupportedContext("open_timeline_for_closed_buffer"))
        }

        /// Open the buffer-timeline overlay for the focused buffer.
        fn open_buffer_timeline(&mut self) -> Result<(), Error> {
            Err(Error::UnsupportedContext("open_buffer_timeline"))
        }

        /// Stamp `label` onto the next snapshot for the active buffer.
        fn mark_next_snapshot(&mut self, _label: &str) -> Result<(), Error> {
            Err(Error::UnsupportedContext("mark_next_snapshot"))
        }

        /// Open or focus the synthetic read-only tutorial tab.
        fn show_tutorial_buffer(&mut self) -> Result<(), Error> {
            Err(Error::UnsupportedContext("show_tutorial_buffer"))
        }

        /// Open or focus the buffer-history swimlane tab.
        fn show_buffer_history_tab(&mut self) -> Result<(), Error> {
            Err(Error::UnsupportedContext("show_buffer_history_tab"))
        }
    };
}
