// Design tokens ported 1:1 from crates/ui-egui/src/theme.rs (Tokens::light / Tokens::dark).
export class Tokens {
  titlebar: string = '';
  chrome: string = '';
  panel: string = '';
  pasteboard: string = '';
  card: string = '';
  border: string = '';
  divider: string = '';
  text: string = '';
  textMuted: string = '';
  textFaint: string = '';
  icon: string = '';
  hover: string = '';
  pressed: string = '';
  selected: string = '';
  accent: string = '';
  accentText: string = '';
  accentSoft: string = '';
  field: string = '';
  radius: number = 6;
}

function light(): Tokens {
  const t = new Tokens();
  t.titlebar = '#E9E9EB'; t.chrome = '#FFFFFF'; t.panel = '#FFFFFF'; t.pasteboard = '#F1F1F3';
  t.card = '#FFFFFF'; t.border = '#DADADE'; t.divider = '#E8E8EB'; t.text = '#222226';
  t.textMuted = '#5E5E66'; t.textFaint = '#6B6B73'; t.icon = '#44444B'; t.hover = '#F0F0F3';
  t.pressed = '#E4E4E9'; t.selected = '#E6EEFD'; t.accent = '#1B63E0'; t.accentText = '#1755C4';
  t.accentSoft = '#E3ECFD'; t.field = '#FFFFFF';
  return t;
}

function dark(): Tokens {
  const t = new Tokens();
  t.titlebar = '#1B1B1E'; t.chrome = '#26262A'; t.panel = '#26262A'; t.pasteboard = '#19191C';
  t.card = '#2E2E33'; t.border = '#3C3C43'; t.divider = '#333339'; t.text = '#ECECEF';
  t.textMuted = '#AEAEB6'; t.textFaint = '#97979E'; t.icon = '#D4D4DA'; t.hover = '#34343A';
  t.pressed = '#3E3E45'; t.selected = '#233A63'; t.accent = '#4B8BF5'; t.accentText = '#8CB6FA';
  t.accentSoft = '#243657'; t.field = '#1E1E22';
  return t;
}

export function tokens(isDark: boolean): Tokens {
  return isDark ? dark() : light();
}

// Fonts registered at startup from rawfile/fonts (Inter, SIL OFL 1.1), as theme.rs installs them.
export const FONT_REGULAR = 'Inter';
export const FONT_MEDIUM = 'Inter-Medium';
export const FONT_SEMIBOLD = 'Inter-SemiBold';
