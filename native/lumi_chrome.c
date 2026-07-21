#include "lumi_chrome.h"

void lumi_state_init(LumiChromeState *state, uint32_t initial_tabs) {
  if (state == 0) {
    return;
  }
  state->tab_count = initial_tabs;
  state->active_tab = 0;
  state->window_maximized = 0;
}

void lumi_state_set_tab_count(LumiChromeState *state, uint32_t tab_count) {
  if (state == 0) {
    return;
  }
  state->tab_count = tab_count;
  if (state->tab_count == 0) {
    state->active_tab = 0;
    return;
  }
  if (state->active_tab >= state->tab_count) {
    state->active_tab = state->tab_count - 1;
  }
}

void lumi_state_set_active_tab(LumiChromeState *state, uint32_t tab_index) {
  if (state == 0 || state->tab_count == 0) {
    return;
  }
  if (tab_index >= state->tab_count) {
    state->active_tab = state->tab_count - 1;
    return;
  }
  state->active_tab = tab_index;
}

void lumi_state_next_tab(LumiChromeState *state) {
  if (state == 0 || state->tab_count == 0) {
    return;
  }
  state->active_tab = (state->active_tab + 1) % state->tab_count;
}

void lumi_state_previous_tab(LumiChromeState *state) {
  if (state == 0 || state->tab_count == 0) {
    return;
  }
  if (state->active_tab == 0) {
    state->active_tab = state->tab_count - 1;
    return;
  }
  state->active_tab -= 1;
}

uint8_t lumi_state_toggle_maximized(LumiChromeState *state) {
  if (state == 0) {
    return 0;
  }
  state->window_maximized = state->window_maximized ? 0 : 1;
  return state->window_maximized;
}

LumiOpacityProfile lumi_default_opacity_profile(void) {
  LumiOpacityProfile profile;
  profile.desktop_alpha = 0;
  profile.window_alpha = 128;
  profile.window_border_alpha = 64;
  profile.titlebar_alpha = 96;
  profile.tabbar_alpha = 108;
  profile.terminal_alpha = 76;
  profile.terminal_border_alpha = 56;
  profile.shadow_alpha = 42;
  return profile;
}
