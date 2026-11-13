param($in, $out, $mode, [double]$angle0 = 0, [double]$noise0 = 0, [double]$light0 = 1, [int]$blur0 = 0, [double]$scale0 = 1, [int]$q0 = 90)
# Herramienta de desarrollo: estropea la imagen de una partitura para probar el OMR (powershell.exe 5.1).
# Uso: omr-degrade.ps1 -in pagina.png -out salida.jpg -mode scan|photo|custom [-angle0 -noise0 -light0 -blur0 -scale0 -q0]
# mode: scan  = 300 dpi, girada 1,2 grados, ruido leve, JPEG q60
#       photo = girada -2,5 grados, desenfocada, luz desigual, ruido, reducida, JPEG q40
Add-Type -AssemblyName System.Drawing
Add-Type -ReferencedAssemblies System.Drawing -TypeDefinition @"
using System; using System.Drawing; using System.Drawing.Imaging; using System.Runtime.InteropServices;
public static class Degrade {
  public static void Apply(Bitmap bmp, double noise, double lightMin, int blurPasses, int seed) {
    int w = bmp.Width, h = bmp.Height;
    var rect = new Rectangle(0, 0, w, h);
    var d = bmp.LockBits(rect, ImageLockMode.ReadWrite, PixelFormat.Format32bppArgb);
    int n = Math.Abs(d.Stride) * h;
    byte[] px = new byte[n];
    Marshal.Copy(d.Scan0, px, 0, n);
    int stride = d.Stride;
    for (int p = 0; p < blurPasses; p++) {
      byte[] copy = (byte[])px.Clone();
      for (int y = 1; y < h - 1; y++) for (int x = 1; x < w - 1; x++) for (int c = 0; c < 3; c++) {
        int s = 0;
        for (int dy = -1; dy <= 1; dy++) for (int dx = -1; dx <= 1; dx++) s += copy[(y + dy) * stride + (x + dx) * 4 + c];
        px[y * stride + x * 4 + c] = (byte)(s / 9);
      }
    }
    var rnd = new Random(seed);
    for (int y = 0; y < h; y++) {
      for (int x = 0; x < w; x++) {
        double light = lightMin + (1 - lightMin) * (0.65 * x / w + 0.35 * (1 - (double)y / h));
        double g = 0; for (int k = 0; k < 4; k++) g += rnd.NextDouble(); g = (g - 2) * 1.7 * noise;
        for (int c = 0; c < 3; c++) {
          int i = y * stride + x * 4 + c;
          double v = px[i] * light + g;
          px[i] = (byte)Math.Max(0, Math.Min(255, v));
        }
      }
    }
    Marshal.Copy(px, 0, d.Scan0, n);
    bmp.UnlockBits(d);
  }
}
"@
$src = [System.Drawing.Bitmap]::FromFile($in)
$w = $src.Width; $h = $src.Height
if ($mode -eq 'scan') { $angle = 1.2; $noise = 9; $light = 0.92; $blur = 0; $q = 60; $scale = 1.0 }
elseif ($mode -eq 'photo') { $angle = -2.5; $noise = 14; $light = 0.62; $blur = 1; $q = 40; $scale = 0.62 }
else { $angle = $angle0; $noise = $noise0; $light = $light0; $blur = $blur0; $q = $q0; $scale = $scale0 }
$bmp = New-Object System.Drawing.Bitmap($w, $h)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.Clear([System.Drawing.Color]::White)
$g.InterpolationMode = 'HighQualityBicubic'
$g.TranslateTransform($w / 2, $h / 2); $g.RotateTransform($angle); $g.TranslateTransform(-$w / 2, -$h / 2)
$g.DrawImage($src, 0, 0, $w, $h)
$g.Dispose(); $src.Dispose()
[Degrade]::Apply($bmp, $noise, $light, $blur, 7)
$final = $bmp
if ($scale -ne 1.0) {
  $nw = [int]($w * $scale); $nh = [int]($h * $scale)
  $final = New-Object System.Drawing.Bitmap($nw, $nh)
  $g2 = [System.Drawing.Graphics]::FromImage($final); $g2.InterpolationMode = 'HighQualityBicubic'; $g2.DrawImage($bmp, 0, 0, $nw, $nh); $g2.Dispose(); $bmp.Dispose()
}
$codec = [System.Drawing.Imaging.ImageCodecInfo]::GetImageEncoders() | Where-Object { $_.MimeType -eq 'image/jpeg' }
$ep = New-Object System.Drawing.Imaging.EncoderParameters(1)
$ep.Param[0] = New-Object System.Drawing.Imaging.EncoderParameter([System.Drawing.Imaging.Encoder]::Quality, [long]$q)
$final.Save($out, $codec, $ep)
$final.Dispose()
"$out ($mode)"
