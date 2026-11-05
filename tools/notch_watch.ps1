# Test helper: samples the Veyra notch at the top centre of the primary screen and prints its
# size (in physical pixels) whenever it changes.
#   width  = black pixels along row $Row
#   height = black pixels down the centre column
# Usage: powershell -ExecutionPolicy Bypass -File tools\notch_watch.ps1 [seconds] [row]
param([double]$Seconds = 8, [int]$Row = 12)
Add-Type -TypeDefinition @'
using System; using System.Runtime.InteropServices;
public static class NW { [DllImport("user32.dll")] public static extern bool SetProcessDPIAware(); }
'@
[NW]::SetProcessDPIAware() | Out-Null
Add-Type -AssemblyName System.Drawing, System.Windows.Forms
$b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
$w = 900; $h = 120; $x0 = [int]($b.Width / 2 - $w / 2)
$bmp = New-Object System.Drawing.Bitmap $w, $h
$g = [System.Drawing.Graphics]::FromImage($bmp)
function Dark($c) { $c.R -lt 8 -and $c.G -lt 8 -and $c.B -lt 8 }
$sw = [Diagnostics.Stopwatch]::StartNew(); $lw = -1; $lh = -1
while ($sw.Elapsed.TotalSeconds -lt $Seconds) {
    $g.CopyFromScreen($x0, 0, 0, 0, $bmp.Size)
    $nw = 0; for ($i = 0; $i -lt $w; $i++) { if (Dark $bmp.GetPixel($i, $Row)) { $nw++ } }
    $nh = 0; for ($j = 0; $j -lt $h; $j++) { if (Dark $bmp.GetPixel([int]($w / 2), $j)) { $nh++ } else { break } }
    if ([math]::Abs($nw - $lw) -gt 2 -or [math]::Abs($nh - $lh) -gt 1) {
        $t = [DateTimeOffset]::Now.ToUnixTimeMilliseconds() % 100000000
        "{0} {1,6} ms  w={2,4}px  h={3,3}px" -f $t, [int]$sw.Elapsed.TotalMilliseconds, $nw, $nh
        $lw = $nw; $lh = $nh
    }
}
