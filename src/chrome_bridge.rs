#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct LumiChromeState {
    active_tab: u32,
    tab_count: u32,
    window_maximized: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct OpacityProfile {
    pub desktop_alpha: u8,
    pub window_alpha: u8,
    pub window_border_alpha: u8,
    pub titlebar_alpha: u8,
    pub tabbar_alpha: u8,
    pub terminal_alpha: u8,
    pub terminal_border_alpha: u8,
    pub shadow_alpha: u8,
}

unsafe extern "C" {
    fn lumi_state_init(state: *mut LumiChromeState, initial_tabs: u32);
    fn lumi_state_set_tab_count(state: *mut LumiChromeState, tab_count: u32);
    fn lumi_state_set_active_tab(state: *mut LumiChromeState, tab_index: u32);
    fn lumi_state_next_tab(state: *mut LumiChromeState);
    fn lumi_state_previous_tab(state: *mut LumiChromeState);
    fn lumi_state_toggle_maximized(state: *mut LumiChromeState) -> u8;
    fn lumi_default_opacity_profile() -> OpacityProfile;
}

pub struct ChromeBridge {
    state: LumiChromeState,
}

impl ChromeBridge {
    pub fn new(initial_tabs: usize) -> Self {
        let mut state = LumiChromeState {
            active_tab: 0,
            tab_count: 0,
            window_maximized: 0,
        };
        // SAFETY: `state` points to valid stack storage and the C function only mutates it.
        unsafe {
            lumi_state_init(&mut state, initial_tabs as u32);
        }
        Self { state }
    }

    pub fn set_tab_count(&mut self, tab_count: usize) {
        // SAFETY: `self.state` is valid mutable storage owned by this wrapper.
        unsafe {
            lumi_state_set_tab_count(&mut self.state, tab_count as u32);
        }
    }

    pub fn set_active_tab(&mut self, tab_index: usize) {
        // SAFETY: `self.state` is valid mutable storage owned by this wrapper.
        unsafe {
            lumi_state_set_active_tab(&mut self.state, tab_index as u32);
        }
    }

    pub fn next_tab(&mut self) {
        // SAFETY: `self.state` is valid mutable storage owned by this wrapper.
        unsafe {
            lumi_state_next_tab(&mut self.state);
        }
    }

    pub fn previous_tab(&mut self) {
        // SAFETY: `self.state` is valid mutable storage owned by this wrapper.
        unsafe {
            lumi_state_previous_tab(&mut self.state);
        }
    }

    pub fn active_tab(&self) -> usize {
        self.state.active_tab as usize
    }

    pub fn toggle_maximized(&mut self) -> bool {
        // SAFETY: `self.state` is valid mutable storage owned by this wrapper.
        unsafe { lumi_state_toggle_maximized(&mut self.state) != 0 }
    }

    pub fn default_opacity_profile() -> OpacityProfile {
        // SAFETY: C function returns a plain value and requires no input pointers.
        unsafe { lumi_default_opacity_profile() }
    }
}

#[cfg(test)]
mod tests {
    use super::ChromeBridge;

    #[test]
    fn initializes_active_tab_to_zero() {
        assert_eq!(ChromeBridge::new(4).active_tab(), 0);
        assert_eq!(ChromeBridge::new(0).active_tab(), 0);
    }

    #[test]
    fn active_tab_clamps_to_last_tab_when_out_of_range() {
        let mut bridge = ChromeBridge::new(3);
        bridge.set_active_tab(99);
        assert_eq!(bridge.active_tab(), 2);
    }

    #[test]
    fn set_active_tab_is_noop_when_there_are_no_tabs() {
        let mut bridge = ChromeBridge::new(0);
        bridge.set_active_tab(2);
        assert_eq!(bridge.active_tab(), 0);
    }

    #[test]
    fn reducing_tab_count_clamps_active_tab() {
        let mut bridge = ChromeBridge::new(5);
        bridge.set_active_tab(4);
        bridge.set_tab_count(3);
        assert_eq!(bridge.active_tab(), 2);
    }

    #[test]
    fn zero_tab_count_resets_active_tab() {
        let mut bridge = ChromeBridge::new(2);
        bridge.set_active_tab(1);
        bridge.set_tab_count(0);
        assert_eq!(bridge.active_tab(), 0);
    }

    #[test]
    fn next_tab_wraps_around_to_first_tab() {
        let mut bridge = ChromeBridge::new(3);
        bridge.set_active_tab(2);
        bridge.next_tab();
        assert_eq!(bridge.active_tab(), 0);
    }

    #[test]
    fn previous_tab_wraps_around_to_last_tab() {
        let mut bridge = ChromeBridge::new(3);
        bridge.previous_tab();
        assert_eq!(bridge.active_tab(), 2);
    }

    #[test]
    fn toggle_maximized_flips_window_state() {
        let mut bridge = ChromeBridge::new(1);
        assert!(bridge.toggle_maximized());
        assert!(!bridge.toggle_maximized());
        assert!(bridge.toggle_maximized());
    }

    #[test]
    fn default_opacity_profile_matches_native_defaults() {
        let profile = ChromeBridge::default_opacity_profile();
        assert_eq!(profile.desktop_alpha, 0);
        assert_eq!(profile.window_alpha, 128);
        assert_eq!(profile.window_border_alpha, 64);
        assert_eq!(profile.titlebar_alpha, 96);
        assert_eq!(profile.tabbar_alpha, 108);
        assert_eq!(profile.terminal_alpha, 76);
        assert_eq!(profile.terminal_border_alpha, 56);
        assert_eq!(profile.shadow_alpha, 42);
    }
}
