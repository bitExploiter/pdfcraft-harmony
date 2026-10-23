// Types for libpdfcraft_ohos.so (crates/ohos-bridge). Errors are thrown as Error objects whose
// message starts with a code: [E_IO], [E_NOT_PDF], [E_BAD_HANDLE], [E_PAGE_RANGE], [E_RENDER].
export interface RenderedPage {
  width: number;
  height: number;
  /** Premultiplied RGBA8, row-major, stride = width * 4. */
  rgba: Uint8Array;
  nonWhite: number;
  /** Render time in Rust (0 when a thumbnail came from the cache). */
  millis: number;
}
export interface PageSize {
  /** Points, after /Rotate. */
  width: number;
  height: number;
}
export const openPath: (path: string) => number;
export const openBytes: (data: ArrayBuffer) => number;
/** Duplicates `fd` and reads it; the caller still closes its own descriptor. */
export const openFd: (fd: number) => number;
export const pageCount: (handle: number) => number;
export const pageSize: (handle: number, page: number) => PageSize;
export const renderPage: (handle: number, page: number, scale: number) => Promise<RenderedPage>;
export const renderThumbnail: (handle: number, page: number, maxSide: number) => Promise<RenderedPage>;
export const renderPageSync: (handle: number, page: number, scale: number) => RenderedPage;
export const close: (handle: number) => boolean;
export const closeAll: () => void;
