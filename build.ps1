# Builds Veyra (release) and stamps the generated logo into veyra.exe as its icon.
# Usage:  powershell -ExecutionPolicy Bypass -File build.ps1
Set-Location $PSScriptRoot
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"

# cargo reports progress on stderr, which Windows PowerShell would otherwise treat as an error
cmd /c "cargo build --release 2>&1"
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
$ErrorActionPreference = "Stop"

$exe = Join-Path $PSScriptRoot "target\release\veyra.exe"
$ico = Join-Path $PSScriptRoot "target\release\veyra.ico"
Start-Process $exe -ArgumentList "--write-icon", "`"$ico`"" -Wait

Add-Type @"
using System;
using System.IO;
using System.Runtime.InteropServices;
public static class IconStamp {
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern IntPtr BeginUpdateResource(string file, bool deleteExisting);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool UpdateResource(IntPtr h, IntPtr type, IntPtr name, ushort lang, byte[] data, uint size);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool EndUpdateResource(IntPtr h, bool discard);

    public static void Apply(string exe, string ico) {
        byte[] f = File.ReadAllBytes(ico);
        int count = BitConverter.ToUInt16(f, 4);
        IntPtr h = BeginUpdateResource(exe, false);
        if (h == IntPtr.Zero) throw new Exception("BeginUpdateResource failed: " + Marshal.GetLastWin32Error());
        // GRPICONDIR + GRPICONDIRENTRY (14 bytes each; same as ICONDIRENTRY but with a 2-byte id)
        var grp = new MemoryStream();
        var w = new BinaryWriter(grp);
        w.Write((ushort)0); w.Write((ushort)1); w.Write((ushort)count);
        for (int i = 0; i < count; i++) {
            int e = 6 + i * 16;
            int size = BitConverter.ToInt32(f, e + 8);
            int off = BitConverter.ToInt32(f, e + 12);
            byte[] img = new byte[size];
            Array.Copy(f, off, img, 0, size);
            if (!UpdateResource(h, (IntPtr)3 /*RT_ICON*/, (IntPtr)(i + 1), 0, img, (uint)size))
                throw new Exception("UpdateResource(RT_ICON) failed: " + Marshal.GetLastWin32Error());
            w.Write(f, e, 12);          // width, height, colors, reserved, planes, bitcount, bytes
            w.Write((ushort)(i + 1));   // resource id
        }
        byte[] g = grp.ToArray();
        if (!UpdateResource(h, (IntPtr)14 /*RT_GROUP_ICON*/, (IntPtr)1, 0, g, (uint)g.Length))
            throw new Exception("UpdateResource(RT_GROUP_ICON) failed: " + Marshal.GetLastWin32Error());
        if (!EndUpdateResource(h, false)) throw new Exception("EndUpdateResource failed: " + Marshal.GetLastWin32Error());
    }
}
"@
[IconStamp]::Apply($exe, $ico)
Write-Host "Built $exe with the Veyra icon."
