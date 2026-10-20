// A thin, safe C# wrapper over pdfcraft_ffi (see ../../include/pdfcraft.h). .NET 7 or later,
// for [LibraryImport], whose generated marshalling needs <AllowUnsafeBlocks>true</AllowUnsafeBlocks>
// in the project (this file has no unsafe block of its own). Copy the native library (pdfcraft_ffi.dll, libpdfcraft_ffi.dylib or
// libpdfcraft_ffi.so) next to your executable, or resolve it with NativeLibrary.SetDllImportResolver.

using System;
using System.Runtime.InteropServices;
using System.Text.Json;
using System.Text.Json.Nodes;

namespace PdfCraft;

/// <summary>A tool call that did not succeed. <see cref="Status"/> is the PDFCRAFT_* code and
/// <see cref="Kind"/> its name (unknown_tool, invalid_args, failed, panic, bad_call).</summary>
public sealed class PdfCraftException(int status, string kind, string message) : Exception(message)
{
    public int Status { get; } = status;
    public string Kind { get; } = kind;
}

/// <summary>An automation session: open documents, their undo history and caches. Calls on one
/// session serialize behind a lock inside the library; use one session per thread for parallel
/// work. Dispose it to close every document without saving.</summary>
public sealed class PdfCraftSession : IDisposable
{
    private readonly SessionHandle _handle;

    /// <param name="root">A folder every path the tools read or write must lie in (relative
    /// paths resolve inside it), or null for no confinement.</param>
    public PdfCraftSession(string? root = null)
    {
        var handle = Native.pdfcraft_new(root, out var error);
        if (handle.IsInvalid)
        {
            throw new PdfCraftException(Native.BadCall, "bad_call", TakeString(error));
        }
        _handle = handle;
    }

    /// <summary>The library version, e.g. "0.8.0".</summary>
    public static string Version => Marshal.PtrToStringUTF8(Native.pdfcraft_version()) ?? "";

    /// <summary>Run one tool (a name from `pdfcraft-cli tools`) and return its first result:
    /// the tool's JSON, or for an image <c>{"image":"png","width":…,"height":…,"base64":…}</c>.
    /// <paramref name="args"/> is serialized to JSON (an anonymous object works well).</summary>
    public JsonNode Call(string tool, object? args = null)
    {
        return CallAll(tool, args)[0] ?? throw new PdfCraftException(Native.Failed, "failed", $"{tool}: empty result");
    }

    /// <summary>Like <see cref="Call"/>, but every result item (most tools return exactly one).</summary>
    public JsonArray CallAll(string tool, object? args = null)
    {
        string? json = args is null ? null : JsonSerializer.Serialize(args);
        var result = Native.pdfcraft_call(_handle, tool, json, out var status);
        var text = TakeString(result);
        var node = JsonNode.Parse(text) ?? throw new PdfCraftException(Native.Failed, "failed", $"{tool}: no JSON result");
        if (status != Native.Ok)
        {
            var error = node["error"];
            var kind = error?["kind"]?.GetValue<string>() ?? "failed";
            var message = error?["message"]?.GetValue<string>() ?? text;
            throw new PdfCraftException(status, kind, message);
        }
        return node["content"] as JsonArray ?? throw new PdfCraftException(Native.Failed, "failed", $"{tool}: no content");
    }

    /// <summary>The PNG bytes of an image result, such as <c>page_render</c>'s.</summary>
    public static byte[] ImageBytes(JsonNode imageResult)
    {
        var base64 = imageResult["base64"]?.GetValue<string>() ?? throw new ArgumentException("not an image result", nameof(imageResult));
        return Convert.FromBase64String(base64);
    }

    public void Dispose() => _handle.Dispose();

    /// <summary>Copy a string the library returned and give its memory back.</summary>
    private static string TakeString(IntPtr s)
    {
        try
        {
            return Marshal.PtrToStringUTF8(s) ?? "";
        }
        finally
        {
            Native.pdfcraft_free_string(s);
        }
    }
}

internal sealed class SessionHandle : SafeHandle
{
    public SessionHandle() : base(IntPtr.Zero, ownsHandle: true) { }
    public override bool IsInvalid => handle == IntPtr.Zero;
    protected override bool ReleaseHandle()
    {
        Native.pdfcraft_free(handle);
        return true;
    }
}

internal static partial class Native
{
    private const string Library = "pdfcraft_ffi";

    public const int Ok = 0;
    public const int UnknownTool = 1;
    public const int InvalidArgs = 2;
    public const int Failed = 3;
    public const int Panic = 4;
    public const int BadCall = 5;

    [LibraryImport(Library)]
    public static partial IntPtr pdfcraft_version();

    [LibraryImport(Library, StringMarshalling = StringMarshalling.Utf8)]
    public static partial SessionHandle pdfcraft_new(string? rootUtf8, out IntPtr error);

    [LibraryImport(Library, StringMarshalling = StringMarshalling.Utf8)]
    public static partial IntPtr pdfcraft_call(SessionHandle session, string tool, string? argsJson, out int status);

    [LibraryImport(Library)]
    public static partial void pdfcraft_free_string(IntPtr s);

    [LibraryImport(Library)]
    public static partial void pdfcraft_free(IntPtr session);
}
