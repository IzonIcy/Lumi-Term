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
