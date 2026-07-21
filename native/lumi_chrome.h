#ifndef LUMI_CHROME_H
#define LUMI_CHROME_H

#include <stdint.h>

typedef struct {
  uint32_t active_tab;
  uint32_t tab_count;
  uint8_t window_maximized;
} LumiChromeState;

typedef struct {
  uint8_t desktop_alpha;
  uint8_t window_alpha;
  uint8_t window_border_alpha;
  uint8_t titlebar_alpha;
  uint8_t tabbar_alpha;
  uint8_t terminal_alpha;
  uint8_t terminal_border_alpha;
  uint8_t shadow_alpha;
} LumiOpacityProfile;

void lumi_state_init(LumiChromeState *state, uint32_t initial_tabs);
void lumi_state_set_tab_count(LumiChromeState *state, uint32_t tab_count);
void lumi_state_set_active_tab(LumiChromeState *state, uint32_t tab_index);
void lumi_state_next_tab(LumiChromeState *state);
void lumi_state_previous_tab(LumiChromeState *state);
uint8_t lumi_state_toggle_maximized(LumiChromeState *state);
LumiOpacityProfile lumi_default_opacity_profile(void);

#endif
