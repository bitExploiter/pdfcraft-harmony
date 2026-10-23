import { hilog } from '@kit.PerformanceAnalysisKit';
import { fileIo as fs } from '@kit.CoreFileKit';
import { image } from '@kit.ImageKit';
import { common } from '@kit.AbilityKit';
import pdf from 'libpdfcraft_ohos.so';

const DOMAIN = 0x0001;
const TAG = 'PdfCraftTest';

@Entry
@Component
struct Index {
  @State status: string = 'Abriendo PDF…';
  @State pages: number = -1;
  @State pixel: image.PixelMap | undefined = undefined;

  aboutToAppear(): void {
    this.runTest();
  }

  private async runTest(): Promise<void> {
    try {
      const ctx = this.getUIContext().getHostContext() as common.UIAbilityContext;
      const data: Uint8Array = ctx.resourceManager.getRawFileContentSync('sample.pdf');
      const path = ctx.filesDir + '/sample.pdf';
      const file = fs.openSync(path, fs.OpenMode.CREATE | fs.OpenMode.READ_WRITE | fs.OpenMode.TRUNC);
      fs.writeSync(file.fd, data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength));
      fs.closeSync(file);

      const handle = pdf.openPath(path);
      this.pages = pdf.pageCount(handle);
      hilog.info(DOMAIN, TAG, 'PDFCRAFT_OK pageCount=%{public}d path=%{public}s', this.pages, path);

      const r = pdf.renderPage(handle, 0, 1.5);
      hilog.info(DOMAIN, TAG, 'PDFCRAFT_RENDER %{public}dx%{public}d bytes=%{public}d', r.width, r.height, r.rgba.byteLength);
      this.pixel = await image.createPixelMap(r.rgba, {
        size: { width: r.width, height: r.height },
        srcPixelFormat: image.PixelMapFormat.RGBA_8888,
        pixelFormat: image.PixelMapFormat.RGBA_8888,
        alphaType: image.AlphaType.PREMUL
      });
      pdf.close(handle);
      this.status = 'Motor PdfCraft (Rust) cargado';
    } catch (e) {
      this.status = 'Error: ' + JSON.stringify(e) + ' ' + String(e);
      hilog.error(DOMAIN, TAG, 'PDFCRAFT_FAIL %{public}s', String(e));
    }
  }

  build() {
    Column({ space: 12 }) {
      Text('PdfCraft · prueba técnica').fontSize(22).fontWeight(FontWeight.Bold)
      Text(this.status).fontSize(16)
      Text(this.pages >= 0 ? `Páginas: ${this.pages}` : '').fontSize(16)
      if (this.pixel) {
        Image(this.pixel)
          .objectFit(ImageFit.Contain)
          .layoutWeight(1)
          .width('100%')
          .border({ width: 1, color: Color.Gray })
      }
    }
    .padding(16)
    .width('100%')
    .height('100%')
    .alignItems(HorizontalAlign.Start)
  }
}
