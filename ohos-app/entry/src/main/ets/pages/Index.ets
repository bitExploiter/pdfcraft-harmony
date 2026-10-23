// PdfCraft viewer: ArkUI port of the original egui window (crates/ui-egui: chrome.rs tab_strip,
// mode_bar and right_rail; panels.rs All tools and Pages; canvas.rs page view and quick tools).
// Sizes are egui points 1:1 as vp; colours and fonts come from Theme.ets (theme.rs).
import { hilog } from '@kit.PerformanceAnalysisKit';
import { fileIo as fs, picker } from '@kit.CoreFileKit';
import { common, ConfigurationConstant } from '@kit.AbilityKit';
import { image } from '@kit.ImageKit';
import { BusinessError } from '@kit.BasicServicesKit';
import { Tokens, tokens, FONT_REGULAR, FONT_MEDIUM, FONT_SEMIBOLD } from '../common/Theme';
import { icon, initIcons } from '../common/Icons';
import * as Pdf from '../common/Pdf';

class Tool {
  label: string;
  icon: string;
  hue: string;
  constructor(label: string, icon: string, hue: string) {
    this.label = label;
    this.icon = icon;
    this.hue = hue;
  }
}

// crates/engine/src/catalog.rs TOOL_GROUPS (first COLLAPSED_TOOLS = 14 shown, then "View more").
const RED = '#E03E3E';
const PURPLE = '#8E4EE6';
const BLUE = '#3A6FE8';
const GREEN = '#2D9D5B';
const ORANGE = '#E88A1A';
const TEAL = '#149CA8';
const PINK = '#D63B8F';
const TOOLS: Tool[] = [
  new Tool('Export a PDF', 'file-output', BLUE), new Tool('Fill & Sign', 'pen-line', PURPLE),
  new Tool('Edit a PDF', 'file-pen-line', PINK), new Tool('Create a PDF', 'file-plus-2', RED),
  new Tool('Combine files', 'files', BLUE), new Tool('Organize pages', 'layout-grid', GREEN),
  new Tool('Add comments', 'message-square-text', ORANGE), new Tool('Scan & OCR', 'scan-text', GREEN),
  new Tool('Protect a PDF', 'shield-check', BLUE), new Tool('Redact a PDF', 'rectangle-horizontal', RED),
  new Tool('Compress a PDF', 'file-down', RED), new Tool('Prepare a form', 'text-cursor-input', PURPLE),
  new Tool('Add a stamp', 'stamp', PURPLE), new Tool('Use a certificate', 'badge-check', TEAL)
];

class RailItem {
  id: string;
  icon: string;
  tip: string;
  constructor(id: string, icon: string, tip: string) {
    this.id = id;
    this.icon = icon;
    this.tip = tip;
  }
}

const RAIL: RailItem[] = [
  new RailItem('comments', 'message-square-text', 'Comments'), new RailItem('bookmarks', 'bookmark', 'Bookmarks'),
  new RailItem('pages', 'files', 'Page thumbnails'), new RailItem('fields', 'text-cursor-input', 'Form fields'),
  new RailItem('layers', 'layers', 'Layers'), new RailItem('attachments', 'paperclip', 'Attachments'),
  new RailItem('signatures', 'signature', 'Signatures'), new RailItem('tags', 'tag', 'Tags')
];

const QUICK: string[] = ['mouse-pointer-2', 'hand', 'message-square-plus', 'highlighter', 'pencil', 'pen-line'];
const PAGE_GAP = 16;
const PAGE_MARGIN = 64;
const THUMB_MAX = 150;

@Entry
@Component
struct Index {
  @State dark: boolean = true;
  @State t: Tokens = tokens(true);
  @State fileName: string = '';
  @State handle: number = 0;
  @State count: number = 0;
  @State dims: Pdf.PageDims[] = [];
  @State indices: number[] = [];
  @State pages: (image.PixelMap | undefined)[] = [];
  @State thumbs: (image.PixelMap | undefined)[] = [];
  @State current: number = 0;
  @State zoom: number = 1;
  @State fitWidth: boolean = true;
  @State rotation: number = 0;
  @State leftOpen: boolean = true;
  @State right: string = '';
  @State quick: number = 0;
  @State canvasW: number = 0;
  @State status: string = '';
  private requested: Set<number> = new Set<number>();
  private visible: Set<number> = new Set<number>();
  private thumbRequested: Set<number> = new Set<number>();
  private scroller: ListScroller = new ListScroller();
  private thumbScroller: ListScroller = new ListScroller();
  private generation: number = 0;

  aboutToAppear(): void {
    const ctx = this.getUIContext().getHostContext() as common.UIAbilityContext;
    initIcons(ctx);
    const font = this.getUIContext().getFont();
    font.registerFont({ familyName: FONT_REGULAR, familySrc: $rawfile('fonts/Inter-Regular.ttf') });
    font.registerFont({ familyName: FONT_MEDIUM, familySrc: $rawfile('fonts/Inter-Medium.ttf') });
    font.registerFont({ familyName: FONT_SEMIBOLD, familySrc: $rawfile('fonts/Inter-SemiBold.ttf') });
    this.dark = ctx.config.colorMode !== ConfigurationConstant.ColorMode.COLOR_MODE_LIGHT;
    this.t = tokens(this.dark);
    this.openBundled(ctx);
  }

  private openBundled(ctx: common.UIAbilityContext): void {
    try {
      const data: Uint8Array = ctx.resourceManager.getRawFileContentSync('sample-20.pdf');
      const path = ctx.filesDir + '/sample-20.pdf';
      const f = fs.openSync(path, fs.OpenMode.CREATE | fs.OpenMode.READ_WRITE | fs.OpenMode.TRUNC);
      fs.writeSync(f.fd, data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength));
      fs.closeSync(f);
      this.load(Pdf.openPath(path), 'sample-20.pdf');
    } catch (e) {
      this.fail(e);
    }
  }

  private async openPicker(): Promise<void> {
    try {
      const opts = new picker.DocumentSelectOptions();
      opts.maxSelectNumber = 1;
      opts.fileSuffixFilters = ['PDF|.pdf'];
      const uris = await new picker.DocumentViewPicker(this.getUIContext().getHostContext() as common.Context).select(opts);
      if (uris.length === 0) {
        return;
      }
      const uri = uris[0];
      const t0 = Date.now();
      const f = fs.openSync(uri, fs.OpenMode.READ_ONLY);
      let h = 0;
      try {
        h = Pdf.openFd(f.fd);
      } finally {
        fs.closeSync(f);
      }
      hilog.info(Pdf.DOMAIN, Pdf.TAG, 'PDFCRAFT_PICKED uri=%{public}s openMs=%{public}d', uri, Date.now() - t0);
      const name = decodeURIComponent(uri.substring(uri.lastIndexOf('/') + 1));
      this.load(h, name);
    } catch (e) {
      this.fail(e);
    }
  }

  private fail(e: Object): void {
    const err = e as BusinessError;
    this.status = String(err.message ?? e);
    hilog.error(Pdf.DOMAIN, Pdf.TAG, 'PDFCRAFT_FAIL %{public}s', this.status);
  }

  private load(h: number, name: string): void {
    if (this.handle !== 0) {
      Pdf.close(this.handle);
    }
    this.generation++;
    this.handle = h;
    this.fileName = name;
    this.count = Pdf.pageCount(h);
    const dims: Pdf.PageDims[] = [];
    const idx: number[] = [];
    for (let i = 0; i < this.count; i++) {
      dims.push(Pdf.pageSize(h, i));
      idx.push(i);
    }
    this.dims = dims;
    this.indices = idx;
    this.pages = new Array<image.PixelMap | undefined>(this.count).fill(undefined);
    this.thumbs = new Array<image.PixelMap | undefined>(this.count).fill(undefined);
    this.requested.clear();
    this.thumbRequested.clear();
    this.visible.clear();
    this.current = 0;
    this.fitWidth = true;
    this.applyFit();
    this.scroller.scrollToIndex(0);
    hilog.info(Pdf.DOMAIN, Pdf.TAG, 'PDFCRAFT_OK file=%{public}s pageCount=%{public}d', name, this.count);
  }

  private pageW(i: number): number {
    const d = this.dims[i];
    return d === undefined ? 0 : (this.rotation % 180 === 0 ? d.width : d.height);
  }

  private pageH(i: number): number {
    const d = this.dims[i];
    return d === undefined ? 0 : (this.rotation % 180 === 0 ? d.height : d.width);
  }

  private applyFit(): void {
    if (this.count === 0 || this.canvasW <= 0) {
      return;
    }
    if (this.fitWidth) {
      this.setZoom((this.canvasW - 2 * PAGE_MARGIN) / Math.max(1, this.pageW(0)));
    }
  }

  private setZoom(z: number): void {
    const clamped = Math.min(64, Math.max(0.08, z));
    if (Math.abs(clamped - this.zoom) < 0.001) {
      return;
    }
    this.zoom = clamped;
    this.generation++;
    this.requested.clear();
    this.pages = new Array<image.PixelMap | undefined>(this.count).fill(undefined);
    // Keep the current page in place across a zoom, as DocView does (goto current, 0).
    this.rerenderVisible();
    const keep = this.current;
    setTimeout((): void => { this.scroller.scrollToIndex(keep, false, ScrollAlign.START); }, 0);
    hilog.info(Pdf.DOMAIN, Pdf.TAG, 'PDFCRAFT_ZOOM %{public}d%%', Math.round(this.zoom * 100));
  }

  // Zoom steps as DocView::zoom_step: the next stop in the preset ladder.
  private zoomStep(zoomIn: boolean): void {
    const stops: number[] = [0.1, 0.25, 0.5, 0.75, 1, 1.25, 1.5, 2, 3, 4, 6, 8, 12, 16, 32, 64];
    this.fitWidth = false;
    let next = this.zoom;
    if (zoomIn) {
      for (const s of stops) {
        if (s > this.zoom + 0.001) {
          next = s;
          break;
        }
      }
    } else {
      for (let i = stops.length - 1; i >= 0; i--) {
        if (stops[i] < this.zoom - 0.001) {
          next = stops[i];
          break;
        }
      }
    }
    this.setZoom(next);
  }

  private request(i: number): void {
    if (this.requested.has(i) || this.handle === 0) {
      return;
    }
    this.requested.add(i);
    const gen = this.generation;
    const scale = this.zoom * this.getUIContext().vp2px(1);
    Pdf.renderPage(this.handle, i, scale).then((r: Pdf.Rendered) => {
      if (gen !== this.generation) {
        return;
      }
      const next = this.pages.slice();
      next[i] = r.pixel;
      this.pages = next;
    }).catch((e: Object) => this.fail(e));
  }

  // A zoom or rotation leaves on-screen pages visible, so no visibility event re-requests them.
  private rerenderVisible(): void {
    this.visible.forEach((v: number) => this.request(v));
  }

  private requestThumb(i: number): void {
    if (this.thumbRequested.has(i) || this.handle === 0) {
      return;
    }
    this.thumbRequested.add(i);
    const px = Math.round(THUMB_MAX * this.getUIContext().vp2px(1));
    Pdf.renderThumbnail(this.handle, i, px).then((r: Pdf.Rendered) => {
      const next = this.thumbs.slice();
      next[i] = r.pixel;
      this.thumbs = next;
    }).catch((e: Object) => this.fail(e));
  }

  private goTo(i: number): void {
    if (this.count === 0) {
      return;
    }
    const p = Math.min(this.count - 1, Math.max(0, i));
    this.current = p;
    this.scroller.scrollToIndex(p, false, ScrollAlign.START);
  }

  // ---- shared widgets (icons::button, widgets::mode_tab, ghost_button) ----

  @Builder
  iconButton(name: string, box: number, selected: boolean, enabled: boolean, action: () => void) {
    Button({ type: ButtonType.Normal, stateEffect: true }) {
      Image(icon(name, !enabled ? this.t.textFaint : (selected ? this.t.accentText : this.t.icon)))
        .width(18).height(18).draggable(false)
    }
    .width(box).height(box)
    .borderRadius(this.t.radius)
    .backgroundColor(selected ? this.t.selected : Color.Transparent)
    .enabled(enabled)
    .opacity(enabled ? 1 : 0.5)
    .onClick(() => action())
  }

  // widgets::mode_tab: label in a 48-pt cell, padding 11; the active tab gets a 2-pt accent bar
  // inset 10 pt from each side.
  private textWidth(text: string, family: string, size: number): number {
    const px = this.getUIContext().getMeasureUtils().measureText({ textContent: text, fontSize: size, fontFamily: family });
    return this.getUIContext().px2vp(px);
  }

  @Builder
  modeTab(label: string, active: boolean) {
    Column() {
      Text(label)
        .fontFamily(active ? FONT_SEMIBOLD : FONT_MEDIUM)
        .fontSize(13.5)
        .fontColor(active ? this.t.text : this.t.textMuted)
        .height(46)
      Row()
        .height(2)
        .width(Math.max(0, this.textWidth(label, active ? FONT_SEMIBOLD : FONT_MEDIUM, 13.5) + 2))
        .backgroundColor(active ? this.t.accent : Color.Transparent)
        .borderRadius({ topLeft: 1, topRight: 1 })
    }
    .height(48)
    .padding({ left: 11, right: 11 })
  }

  // ---- chrome.rs tab_strip (TAB_STRIP 38, TAB_HEIGHT 30, STRIP_BUTTON 28) ----

  @Builder
  tabStrip() {
    Row({ space: 4 }) {
      this.iconButton('house', 28, false, true, () => {})
      Row({ space: 6 }) {
        Image(icon('file-text', this.t.accent)).width(16).height(16)
        Text(this.fileName).fontFamily(FONT_MEDIUM).fontSize(13).fontColor(this.t.text)
          .maxLines(1).textOverflow({ overflow: TextOverflow.Ellipsis }).constraintSize({ maxWidth: 200 })
        Blank().width(8)
        this.iconButton('x', 20, false, true, () => {})
      }
      .height(30).padding({ left: 10, right: 4 })
      .backgroundColor(this.t.chrome)
      .borderRadius({ topLeft: 8, topRight: 8 })
      .alignSelf(ItemAlign.End)
      Row({ space: 6 }) {
        Image(icon('plus', this.t.icon)).width(16).height(16)
        Text('Open').fontFamily(FONT_MEDIUM).fontSize(13).fontColor(this.t.text)
      }
      .height(28).padding({ left: 8, right: 8 }).borderRadius(this.t.radius)
      .onClick(() => { this.openPicker(); })
      Blank()
      Row({ space: 6 }) {
        Image(icon('messages-square', this.t.icon)).width(16).height(16)
        Text('Discord').fontFamily(FONT_MEDIUM).fontSize(13).fontColor(this.t.text)
      }.height(28).padding({ left: 8, right: 8 })
      this.iconButton('circle-help', 28, false, true, () => {})
      this.iconButton(this.dark ? 'moon' : 'sun', 28, false, true, () => {
        this.dark = !this.dark;
        this.t = tokens(this.dark);
      })
    }
    .width('100%').height(38)
    .padding({ left: 8, right: 10 })
    .backgroundColor(this.t.titlebar)
  }

  // ---- chrome.rs mode_bar (48) ----

  @Builder
  modeBar() {
    Row({ space: 2 }) {
      Row({ space: 6 }) {
        Image(icon('panel-left', this.t.icon)).width(18).height(18)
        Text('Menu').fontFamily(FONT_MEDIUM).fontSize(13.5).fontColor(this.t.text)
      }
      .height(32).padding({ left: 8, right: 8 }).borderRadius(this.t.radius)
      .onClick(() => { this.leftOpen = !this.leftOpen; })
      Divider().vertical(true).height(24).color(this.t.divider).margin({ left: 6, right: 10 })
      this.modeTab('All tools', true)
      this.modeTab('Edit', false)
      this.modeTab('Convert', false)
      this.modeTab('E-Sign', false)
      Blank()
      Row({ space: 8 }) {
        Image(icon('search', this.t.textFaint)).width(15).height(15)
        Text('Find tools and commands').fontFamily(FONT_REGULAR).fontSize(13).fontColor(this.t.textFaint)
          .layoutWeight(1).maxLines(1)
        Text('Ctrl+K').fontFamily(FONT_REGULAR).fontSize(11.5).fontColor(this.t.textFaint)
      }
      .width(260).height(30).padding({ left: 10, right: 10 })
      .borderRadius(15).border({ width: 1, color: this.t.border }).backgroundColor(this.t.field)
      Blank().width(8)
      this.iconButton('info', 32, false, true, () => {})
      this.iconButton('save', 32, false, true, () => {})
      this.iconButton('printer', 32, false, true, () => {})
    }
    .width('100%').height(48)
    .padding({ left: 10, right: 10 })
    .backgroundColor(this.t.chrome)
    .border({ width: { top: 1, bottom: 1 }, color: this.t.divider })
  }

  // ---- panels.rs all_tools ----

  @Builder
  allTools() {
    Column() {
      Row() {
        Blank().width(26)
        Text('All tools').fontFamily(FONT_SEMIBOLD).fontSize(15.5).fontColor(this.t.text)
          .layoutWeight(1).textAlign(TextAlign.Center)
        this.iconButton('x', 26, false, true, () => { this.leftOpen = false; })
      }.width('100%').height(26)
      Divider().color(this.t.divider).margin({ top: 10, bottom: 8 })
      Scroll() {
        Column() {
          ForEach(TOOLS, (g: Tool) => {
            Row() {
              Image(icon(g.icon, g.hue)).width(19).height(19).margin({ left: 7, right: 10 })
              Text(g.label).fontFamily(FONT_REGULAR).fontSize(13.5).fontColor(this.t.text)
                .maxLines(1).textOverflow({ overflow: TextOverflow.Ellipsis }).layoutWeight(1)
            }
            .width('100%').height(34).borderRadius(6)
          }, (g: Tool) => g.label)
          Text('View more').fontFamily(FONT_MEDIUM).fontSize(13).fontColor(this.t.accentText)
            .margin({ top: 4 }).alignSelf(ItemAlign.Start)
        }.width('100%')
      }.layoutWeight(1).scrollBar(BarState.Off).align(Alignment.Top)
    }
    .width(242).height('100%')
    .padding({ left: 14, right: 10, top: 12, bottom: 10 })
    .backgroundColor(this.t.panel)
    .border({ width: { right: 1 }, color: this.t.divider })
  }

  // ---- canvas.rs: pasteboard, pages, quick tools card ----

  @Builder
  canvas() {
    Stack({ alignContent: Alignment.TopStart }) {
      List({ space: PAGE_GAP, scroller: this.scroller }) {
        ForEach(this.indices, (i: number) => {
          ListItem() {
            Stack() {
              if (this.pages[i] !== undefined) {
                Image(this.pages[i]).width('100%').height('100%').objectFit(ImageFit.Fill)
                  .rotate({ angle: this.rotation }).interpolation(ImageInterpolation.High).draggable(false)
              }
            }
            .width(this.pageW(i) * this.zoom).height(this.pageH(i) * this.zoom)
            .backgroundColor(Color.White)
            .shadow({ radius: 8, color: '#40000000', offsetY: 2 })
            .onVisibleAreaChange([0.0], (visible: boolean) => {
              if (visible) {
                this.visible.add(i);
                this.request(i);
              } else {
                this.visible.delete(i);
              }
            })
          }
        }, (i: number) => `${i}`)
      }
      .width('100%').height('100%')
      .alignListItem(ListItemAlign.Center)
      .padding({ top: 24, bottom: 24 })
      .cachedCount(1)
      .scrollBar(BarState.Auto)
      .onScrollIndex((first: number) => { this.current = first; })

      // Quick tools: Frame card, corner radius 12, margin 6, 32-pt buttons (canvas.rs).
      Column({ space: 2 }) {
        ForEach(QUICK, (name: string, k: number) => {
          Stack() {
            this.iconButton(name, 32, this.quick === k, true, () => { this.quick = k; })
          }
          .border(this.quick === k ? { width: 1, color: this.t.accent, radius: this.t.radius } : { width: 0 })
        }, (name: string) => name)
      }
      .padding(6).margin({ left: 12, top: 12 })
      .backgroundColor(this.t.card).borderRadius(12)
      .border({ width: 1, color: this.t.border })
      .shadow({ radius: 6, color: '#33000000', offsetY: 1 })
    }
    .layoutWeight(1).height('100%')
    .backgroundColor(this.t.pasteboard)
    .clip(true)
    .onAreaChange((_o: Area, n: Area) => {
      this.canvasW = Number(n.width);
      this.applyFit();
    })
  }

  // ---- panels.rs pages (right panel, default 330) ----

  @Builder
  pagesPanel() {
    Column() {
      Row({ space: 8 }) {
        Text('Pages').fontFamily(FONT_SEMIBOLD).fontSize(15.5).fontColor(this.t.text)
        Text(`${this.count}`).fontFamily(FONT_MEDIUM).fontSize(13).fontColor(this.t.textFaint)
        Blank()
        this.iconButton('x', 26, false, true, () => { this.right = ''; })
      }.width('100%').height(26)
      List({ space: 4, scroller: this.thumbScroller }) {
        ForEach(this.indices, (i: number) => {
          ListItem() {
            Column({ space: 4 }) {
              Stack() {
                if (this.thumbs[i] !== undefined) {
                  Image(this.thumbs[i]).width('100%').height('100%').objectFit(ImageFit.Fill).draggable(false)
                }
              }
              .width(THUMB_MAX).height(THUMB_MAX * this.pageH(i) / Math.max(1, this.pageW(i)))
              .backgroundColor(Color.White)
              .outline({ width: i === this.current ? 2 : 1, color: i === this.current ? this.t.accent : this.t.border })
              .onVisibleAreaChange([0.0], (visible: boolean) => {
                if (visible) {
                  this.requestThumb(i);
                }
              })
              Text(`${i + 1}`).fontFamily(FONT_MEDIUM).fontSize(12)
                .fontColor(i === this.current ? this.t.accentText : this.t.textMuted)
            }
            .padding(8).borderRadius(8)
            .backgroundColor(i === this.current ? this.t.selected : Color.Transparent)
            .onClick(() => this.goTo(i))
          }
        }, (i: number) => `${i}`)
      }
      .layoutWeight(1).width('100%').alignListItem(ListItemAlign.Center).margin({ top: 10 })
      .cachedCount(2)
    }
    .width(330).height('100%')
    .padding({ left: 14, right: 12, top: 12, bottom: 8 })
    .backgroundColor(this.t.panel)
    .border({ width: { left: 1 }, color: this.t.divider })
  }

  // ---- chrome.rs right_rail (48) ----

  @Builder
  rail() {
    Column() {
      Column({ space: 4 }) {
        ForEach(RAIL, (r: RailItem) => {
          this.iconButton(r.icon, 34, this.right === r.id, true, () => {
            this.right = this.right === r.id ? '' : r.id;
          })
        }, (r: RailItem) => r.id)
      }
      Blank()
      Column({ space: 2 }) {
        TextInput({ text: `${this.current + 1}` })
          .width(34).height(26).padding({ left: 2, right: 2 })
          .fontFamily(FONT_MEDIUM).fontSize(12).fontColor(this.t.text).textAlign(TextAlign.Center)
          .backgroundColor(this.t.field).borderRadius(5).border({ width: 1, color: this.t.border })
          .type(InputType.Number)
          .onSubmit((_k: EnterKeyType, ev: SubmitEvent) => { this.goTo(Number(ev.text) - 1); })
        Text(`${this.count}`).fontFamily(FONT_REGULAR).fontSize(11).fontColor(this.t.textMuted)
        this.iconButton('chevron-left', 30, false, false, () => {})
        this.iconButton('chevron-right', 30, false, false, () => {})
        this.iconButton('chevron-up', 30, false, true, () => this.goTo(this.current - 1))
        this.iconButton('chevron-down', 30, false, true, () => this.goTo(this.current + 1))
        Text(`${Math.round(this.zoom * 100)}%`).fontFamily(FONT_MEDIUM).fontSize(11).fontColor(this.t.textMuted)
          .margin({ top: 6 })
        this.iconButton(this.fitWidth ? 'maximize' : 'maximize-2', 32, false, true, () => {
          this.fitWidth = !this.fitWidth;
          if (this.fitWidth) {
            this.applyFit();
          } else {
            const h = this.canvasW > 0 ? this.canvasW : 800;
            this.setZoom(Math.min(this.zoom, (h - 2 * PAGE_MARGIN) / Math.max(1, this.pageH(0))));
          }
        })
        this.iconButton('rotate-cw', 32, false, true, () => {
          this.rotation = (this.rotation + 90) % 360;
          this.generation++;
          this.requested.clear();
          this.pages = new Array<image.PixelMap | undefined>(this.count).fill(undefined);
          this.applyFit();
          this.rerenderVisible();
        })
        this.iconButton('file-text', 32, false, true, () => {})
        this.iconButton('zoom-in', 32, false, true, () => this.zoomStep(true))
        this.iconButton('zoom-out', 32, false, true, () => this.zoomStep(false))
      }
      .alignItems(HorizontalAlign.Center)
    }
    .width(48).height('100%')
    .padding({ left: 6, right: 6, top: 8, bottom: 8 })
    .backgroundColor(this.t.chrome)
    .border({ width: { left: 1 }, color: this.t.divider })
  }

  build() {
    Column() {
      this.tabStrip()
      this.modeBar()
      Row() {
        if (this.leftOpen) {
          this.allTools()
        }
        this.canvas()
        if (this.right === 'pages') {
          this.pagesPanel()
        }
        this.rail()
      }
      .layoutWeight(1).width('100%')
    }
    .width('100%').height('100%')
    .backgroundColor(this.t.chrome)
  }
}
