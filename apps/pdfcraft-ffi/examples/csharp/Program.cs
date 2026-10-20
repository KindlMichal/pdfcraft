// PNG (or JPEG, TIFF, GIF, BMP) → PDF through pdfcraft_ffi:
//   dotnet run -- picture.png picture.pdf
// Paths are confined to the current directory (the session's root).

using PdfCraft;

if (args.Length != 2)
{
    Console.Error.WriteLine("usage: PdfCraft.Sample <image> <out.pdf>");
    return 2;
}

using var session = new PdfCraftSession(root: Environment.CurrentDirectory);
try
{
    var created = session.Call("doc_create", new { from = "images", paths = new[] { args[0] }, dpi = 72 });
    var doc = created["doc"]!.GetValue<ulong>();
    var saved = session.Call("doc_save", new { doc, path = args[1] });
    Console.WriteLine($"PdfCraft {PdfCraftSession.Version}: {created["pages"]} page(s) written to {saved["path"]}");
    return 0;
}
catch (PdfCraftException e)
{
    Console.Error.WriteLine($"{e.Kind} ({e.Status}): {e.Message}");
    return 1;
}
