Add-Type -AssemblyName System.Drawing

$size = 512
$bmp = New-Object System.Drawing.Bitmap($size, $size)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
$g.Clear([System.Drawing.Color]::Transparent)

$path = New-Object System.Drawing.Drawing2D.GraphicsPath
$inset = 22
$r = 112
$x = $inset; $y = $inset
$w = $size - 2 * $inset; $h = $size - 2 * $inset
$path.AddArc($x, $y, $r, $r, 180, 90)
$path.AddArc($x + $w - $r, $y, $r, $r, 270, 90)
$path.AddArc($x + $w - $r, $y + $h - $r, $r, $r, 0, 90)
$path.AddArc($x, $y + $h - $r, $r, $r, 90, 90)
$path.CloseFigure()

$bg = [System.Drawing.Drawing2D.LinearGradientBrush]::new(
  [System.Drawing.Point]::new(0, 0),
  [System.Drawing.Point]::new($size, $size),
  [System.Drawing.Color]::FromArgb(255, 28, 28, 38),
  [System.Drawing.Color]::FromArgb(255, 9, 9, 12))
$g.FillPath($bg, $path)

$pts = @(
  [System.Drawing.Point]::new(296, 64),
  [System.Drawing.Point]::new(152, 288),
  [System.Drawing.Point]::new(240, 288),
  [System.Drawing.Point]::new(208, 448),
  [System.Drawing.Point]::new(360, 224),
  [System.Drawing.Point]::new(272, 224)
)
$bolt = [System.Drawing.Drawing2D.LinearGradientBrush]::new(
  [System.Drawing.Point]::new(170, 80),
  [System.Drawing.Point]::new(350, 440),
  [System.Drawing.Color]::FromArgb(255, 196, 181, 253),
  [System.Drawing.Color]::FromArgb(255, 126, 20, 255))
$g.FillPolygon($bolt, $pts)

$g.Dispose()
$out = 'D:\MyProject\LangChain\frontend\electron\icon.png'
$bmp.Save($out, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()

$check = [System.Drawing.Image]::FromFile($out)
"Saved $out ($($check.Width)x$($check.Height))"
$check.Dispose()
