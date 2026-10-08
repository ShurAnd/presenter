param($Out)
# Presentator icon: blue rounded tile, white presentation screen on a tripod, green play button.
Add-Type -AssemblyName System.Drawing
New-Item -ItemType Directory -Force $Out | Out-Null

function RoundRect([float]$x, [float]$y, [float]$w, [float]$h, [float]$r) {
  $p = New-Object System.Drawing.Drawing2D.GraphicsPath
  $d = 2 * $r
  $p.AddArc($x, $y, $d, $d, 180, 90); $p.AddArc($x + $w - $d, $y, $d, $d, 270, 90)
  $p.AddArc($x + $w - $d, $y + $h - $d, $d, $d, 0, 90); $p.AddArc($x, $y + $h - $d, $d, $d, 90, 90)
  $p.CloseFigure(); return $p
}
function C($hex, $a = 255) { $c = [System.Drawing.ColorTranslator]::FromHtml($hex); [System.Drawing.Color]::FromArgb($a, $c) }

foreach ($n in 16, 20, 24, 32, 40, 48, 64, 128, 256) {
  $bmp = New-Object System.Drawing.Bitmap $n, $n, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.SmoothingMode = 'AntiAlias'; $g.PixelOffsetMode = 'HighQuality'; $g.Clear([System.Drawing.Color]::Transparent)
  $s = $n / 256.0
  $small = $n -le 24

  # Tile: rounded square with vertical gradient.
  $m = [Math]::Max(0.5, 8 * $s)
  $tile = RoundRect $m $m ($n - 2 * $m) ($n - 2 * $m) (52 * $s)
  $grad = New-Object System.Drawing.Drawing2D.LinearGradientBrush (New-Object System.Drawing.PointF 0, 0), (New-Object System.Drawing.PointF 0, $n), (C '#3A9BF0'), (C '#1653A8')
  $g.FillPath($grad, $tile)

  $white = New-Object System.Drawing.SolidBrush (C '#FFFFFF')
  $legPen = New-Object System.Drawing.Pen (C '#FFFFFF'), ([Math]::Max(1.6, 13 * $s))
  $legPen.StartCap = 'Round'; $legPen.EndCap = 'Round'

  if ($small) {
    # Screen + one stand line.
    $scr = RoundRect (40 * $s) (52 * $s) (176 * $s) (118 * $s) (14 * $s)
    $g.DrawLine($legPen, 128 * $s, 165 * $s, 128 * $s, 214 * $s)
    $g.FillPath($white, $scr)
    $tri = @((New-Object System.Drawing.PointF (106 * $s), (80 * $s)), (New-Object System.Drawing.PointF (106 * $s), (142 * $s)), (New-Object System.Drawing.PointF (160 * $s), (111 * $s)))
  } else {
    # Tripod legs behind the screen.
    $g.DrawLine($legPen, 128 * $s, 160 * $s, 82 * $s, 214 * $s)
    $g.DrawLine($legPen, 128 * $s, 160 * $s, 174 * $s, 214 * $s)
    $g.DrawLine($legPen, 128 * $s, 160 * $s, 128 * $s, 206 * $s)
    # Screen shadow, screen, top roller bar.
    $sh = RoundRect (46 * $s) (60 * $s) (164 * $s) (112 * $s) (12 * $s)
    $g.FillPath((New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(70, 0, 20, 60))), $sh)
    $scr = RoundRect (44 * $s) (54 * $s) (168 * $s) (112 * $s) (12 * $s)
    $g.FillPath($white, $scr)
    $bar = RoundRect (34 * $s) (44 * $s) (188 * $s) (18 * $s) (9 * $s)
    $g.FillPath((New-Object System.Drawing.SolidBrush (C '#E3ECF8')), $bar)
    # Small "slide" bars on the left of the screen.
    $lb = New-Object System.Drawing.SolidBrush (C '#B9CDE8')
    $g.FillPath($lb, (RoundRect (62 * $s) (82 * $s) (54 * $s) (12 * $s) (6 * $s)))
    $g.FillPath($lb, (RoundRect (62 * $s) (104 * $s) (40 * $s) (12 * $s) (6 * $s)))
    $g.FillPath($lb, (RoundRect (62 * $s) (126 * $s) (48 * $s) (12 * $s) (6 * $s)))
    $tri = @((New-Object System.Drawing.PointF (136 * $s), (82 * $s)), (New-Object System.Drawing.PointF (136 * $s), (144 * $s)), (New-Object System.Drawing.PointF (188 * $s), (113 * $s)))
  }
  # Green play triangle with rounded joins.
  $green = C '#2DBE5A'
  $tp = New-Object System.Drawing.Drawing2D.GraphicsPath; $tp.AddPolygon($tri)
  $g.FillPath((New-Object System.Drawing.SolidBrush $green), $tp)
  $tpen = New-Object System.Drawing.Pen $green, ([Math]::Max(1, 8 * $s)); $tpen.LineJoin = 'Round'
  $g.DrawPath($tpen, $tp)

  $g.Dispose()
  $bmp.Save((Join-Path $Out "icon_$n.png"), [System.Drawing.Imaging.ImageFormat]::Png)
  $bmp.Dispose()
}
"done"
