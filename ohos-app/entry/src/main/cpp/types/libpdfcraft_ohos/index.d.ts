export interface RenderedPage { width: number; height: number; rgba: ArrayBuffer; }
export const openPath: (path: string) => number;
export const openBytes: (data: ArrayBuffer) => number;
export const pageCount: (handle: number) => number;
export const renderPage: (handle: number, page: number, scale: number) => RenderedPage;
export const close: (handle: number) => void;
