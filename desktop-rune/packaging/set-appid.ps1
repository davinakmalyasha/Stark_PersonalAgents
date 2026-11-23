param(
    [Parameter(Mandatory = $true)][string]$Shortcut,
    [Parameter(Mandatory = $true)][string]$AppId
)

$ErrorActionPreference = "Stop"

# Set System.AppUserModel.ID on a .lnk so the pinned/desktop shortcut groups
# with the running Rune window instead of appearing as a second app.
$source = @'
using System;
using System.Runtime.InteropServices;

public static class StarkShortcutAppId
{
    [StructLayout(LayoutKind.Sequential, Pack = 4)]
    private struct PropertyKey
    {
        public Guid FormatId;
        public uint PropertyId;
    }

    [StructLayout(LayoutKind.Explicit)]
    private struct PropVariant
    {
        [FieldOffset(0)] public ushort VariantType;
        [FieldOffset(2)] public ushort Reserved1;
        [FieldOffset(4)] public ushort Reserved2;
        [FieldOffset(6)] public ushort Reserved3;
        [FieldOffset(8)] public IntPtr PointerValue;
    }

    [ComImport]
    [Guid("886D8EEB-8CF2-4446-8D02-CDBA1DBDCF99")]
    [InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    private interface IPropertyStore
    {
        void GetCount(out uint propertyCount);
        void GetAt(uint propertyIndex, out PropertyKey key);
        void GetValue(ref PropertyKey key, out PropVariant value);
        void SetValue(ref PropertyKey key, ref PropVariant value);
        void Commit();
    }

    [DllImport("shell32.dll", CharSet = CharSet.Unicode, PreserveSig = false)]
    private static extern void SHGetPropertyStoreFromParsingName(
        [MarshalAs(UnmanagedType.LPWStr)] string path,
        IntPtr parent,
        uint flags,
        ref Guid interfaceId,
        [MarshalAs(UnmanagedType.Interface)] out IPropertyStore store);

    public static void Set(string shortcut, string appId)
    {
        Guid interfaceId = typeof(IPropertyStore).GUID;
        IPropertyStore store;
        SHGetPropertyStoreFromParsingName(shortcut, IntPtr.Zero, 2u, ref interfaceId, out store);
        var key = new PropertyKey
        {
            FormatId = new Guid("9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3"),
            PropertyId = 5
        };
        var value = new PropVariant
        {
            VariantType = 31,
            PointerValue = Marshal.StringToCoTaskMemUni(appId)
        };
        try
        {
            store.SetValue(ref key, ref value);
            store.Commit();
        }
        finally
        {
            Marshal.FreeCoTaskMem(value.PointerValue);
            Marshal.ReleaseComObject(store);
        }
    }
}
'@

Add-Type -TypeDefinition $source -Language CSharp
[StarkShortcutAppId]::Set($Shortcut, $AppId)
