// Lucide icons (ISC, rawfile/icons/LICENSE-lucide.txt), tinted like crates/ui-egui/src/icons.rs:
// the SVG's currentColor is replaced with the token colour and the result shown as a data URI.
import { common } from '@kit.AbilityKit';
import { util } from '@kit.ArkTS';

const cache: Map<string, string> = new Map<string, string>();
const sources: Map<string, string> = new Map<string, string>();
let ctx: common.Context | undefined = undefined;

export function initIcons(c: common.Context): void {
  ctx = c;
}

export function icon(name: string, color: string): string {
  const key = name + color;
  const hit = cache.get(key);
  if (hit !== undefined) {
    return hit;
  }
  let svg = sources.get(name);
  if (svg === undefined && ctx !== undefined) {
    const bytes = ctx.resourceManager.getRawFileContentSync('icons/' + name + '.svg');
    svg = util.TextDecoder.create('utf-8').decodeToString(bytes);
    sources.set(name, svg);
  }
  if (svg === undefined) {
    return '';
  }
  const tinted = svg.split('currentColor').join(color);
  const b64 = new util.Base64Helper().encodeToStringSync(new util.TextEncoder().encodeInto(tinted));
  const uri = 'data:image/svg+xml;base64,' + b64;
  cache.set(key, uri);
  return uri;
}
