param($pdf,$out,$width=1800,$x=0,$y=0,$w=0,$h=0,$pg=0)
$ErrorActionPreference='Stop'
Add-Type -AssemblyName System.Runtime.WindowsRuntime,System.Drawing
$null=[Windows.Data.Pdf.PdfDocument,Windows.Data.Pdf,ContentType=WindowsRuntime]
$null=[Windows.Storage.StorageFile,Windows.Storage,ContentType=WindowsRuntime]
$null=[Windows.Storage.Streams.InMemoryRandomAccessStream,Windows.Storage.Streams,ContentType=WindowsRuntime]
$asTask=([System.WindowsRuntimeSystemExtensions].GetMethods()|?{$_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1'})[0]
function Await($op,$type){ $t=$asTask.MakeGenericMethod($type).Invoke($null,@($op)); $t.Wait(-1)|Out-Null; $t.Result }
$file=Await ([Windows.Storage.StorageFile]::GetFileFromPathAsync($pdf)) ([Windows.Storage.StorageFile])
$doc=Await ([Windows.Data.Pdf.PdfDocument]::LoadFromFileAsync($file)) ([Windows.Data.Pdf.PdfDocument])
$page=$doc.GetPage($pg)
$ms=New-Object Windows.Storage.Streams.InMemoryRandomAccessStream
$opt=New-Object Windows.Data.Pdf.PdfPageRenderOptions; $opt.DestinationWidth=[uint32]$width
$asAction=([System.WindowsRuntimeSystemExtensions].GetMethods()|?{$_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncAction'})[0]
$t=$asAction.Invoke($null,@($page.RenderToStreamAsync($ms,$opt))); $t.Wait(-1)|Out-Null
$s=[System.IO.WindowsRuntimeStreamExtensions]::AsStreamForRead($ms)
$bmp=[Drawing.Bitmap]::FromStream($s)
if($w -gt 0){ $bmp=$bmp.Clone((New-Object Drawing.Rectangle $x,$y,$w,$h),$bmp.PixelFormat) }
$bmp.Save($out); $out
