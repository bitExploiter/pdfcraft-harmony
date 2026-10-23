// Thin typed layer over libpdfcraft_ohos.so (crates/ohos-bridge).
import pdf from 'libpdfcraft_ohos.so';
import { RenderedPage } from 'libpdfcraft_ohos.so';
import { image } from '@kit.ImageKit';
import { hilog } from '@kit.PerformanceAnalysisKit';

export const DOMAIN = 0x0001;
export const TAG = 'PdfCraftTest';

export class PageDims {
  width: number = 0;
  height: number = 0;
}

export class Rendered {
  pixel: image.PixelMap | undefined = undefined;
  width: number = 0;
  height: number = 0;
  millis: number = 0;
}

export function openFd(fd: number): number {
  return pdf.openFd(fd);
}

export function openPath(path: string): number {
  return pdf.openPath(path);
}

export function pageCount(h: number): number {
  return pdf.pageCount(h);
}

export function pageSize(h: number, page: number): PageDims {
  const s = pdf.pageSize(h, page);
  const d = new PageDims();
  d.width = s.width;
  d.height = s.height;
  return d;
}

export function close(h: number): void {
  pdf.close(h);
}

async function toPixelMap(r: RenderedPage): Promise<image.PixelMap> {
  const buf = r.rgba.buffer.slice(r.rgba.byteOffset, r.rgba.byteOffset + r.rgba.byteLength);
  return image.createPixelMap(buf, {
    size: { width: r.width, height: r.height },
    srcPixelFormat: image.PixelMapFormat.RGBA_8888,
    pixelFormat: image.PixelMapFormat.RGBA_8888,
    alphaType: image.AlphaType.PREMUL
  });
}

export async function renderPage(h: number, page: number, scale: number): Promise<Rendered> {
  const t0 = Date.now();
  const r = await pdf.renderPage(h, page, scale);
  const out = new Rendered();
  out.pixel = await toPixelMap(r);
  out.width = r.width;
  out.height = r.height;
  out.millis = r.millis;
  hilog.info(DOMAIN, TAG, 'PDFCRAFT_PAGE page=%{public}d %{public}dx%{public}d rustMs=%{public}d totalMs=%{public}d',
    page + 1, r.width, r.height, r.millis, Date.now() - t0);
  return out;
}

export async function renderThumbnail(h: number, page: number, maxSide: number): Promise<Rendered> {
  const t0 = Date.now();
  const r = await pdf.renderThumbnail(h, page, maxSide);
  const out = new Rendered();
  out.pixel = await toPixelMap(r);
  out.width = r.width;
  out.height = r.height;
  out.millis = r.millis;
  hilog.info(DOMAIN, TAG, 'PDFCRAFT_THUMB page=%{public}d %{public}dx%{public}d rustMs=%{public}d totalMs=%{public}d',
    page + 1, r.width, r.height, r.millis, Date.now() - t0);
  return out;
}
