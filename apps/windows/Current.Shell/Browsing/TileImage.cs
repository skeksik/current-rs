using Current.Bindings;
using Microsoft.UI.Xaml.Media.Imaging;

namespace Current.Shell.Browsing;

/// <summary>
/// Loads the images of one tile or picture (ADR 0018, section 4): the core hands the encoded bytes, which become a
/// <see cref="BitmapImage"/> decoded at the box's width. A new load cancels the one before; <see cref="Recycle"/> cancels when the
/// tile is recycled, so a scrolled-past tile stops its fetch. Use it on the UI thread.
/// </summary>
/// <remarks>
/// BitmapImage (learn.microsoft.com, updated 2026-07-28, read 2026-10-25): "You can define a BitmapImage by using a Uniform Resource
/// Identifier (URI) that references an image source file, or by calling SetSourceAsync and supplying a stream"; it reads JPEG, PNG and
/// BMP among others. BitmapImage.DecodePixelType: "The default value is Physical", so DecodePixelWidth is in physical pixels, as the
/// request's width is; the class page's example sets only the width: "You don't need to set Height; the system maintains aspect
/// ratio". WindowsRuntimeStreamExtensions.AsRandomAccessStream(Stream) (learn.microsoft.com, ms.date 2025-07-01, read 2026-10-25):
/// "Converts the specified stream to a random access stream"; that page documents it for UWP .NET; its use from this app's
/// net10.0-windows target is not verified beyond CI's build.
/// </remarks>
public sealed class TileImage(CoreHost core) : IDisposable
{
    /// <summary>
    /// How many decoded pictures stay at hand on the UI thread, the most recently shown, so a page shown again (Home after a detail
    /// page, a recycled tile scrolled back) sets its pictures in the same frame instead of blinking empty, then blurred, then sharp
    /// (owner's run, 2026-10-25). The core's memory cache still holds the bytes behind them; this saves the round trip and the decode.
    /// 150 posters of 240 by 360 pixels are about 50 MB decoded, if XAML keeps them all decoded (not measured).
    /// </summary>
    private const int RecentCapacity = 150;

    private static readonly Dictionary<ImageRequest, LinkedListNode<(ImageRequest Request, BitmapImage Bitmap)>> Recent = [];
    private static readonly LinkedList<(ImageRequest Request, BitmapImage Bitmap)> RecentOrder = new();

    private CancellationTokenSource? _load;

    /// <summary>The picture for <paramref name="request"/> if it was decoded recently; now the most recent. UI thread only.</summary>
    public static BitmapImage? TryRecent(ImageRequest request)
    {
        if (!Recent.TryGetValue(request, out var node))
        {
            return null;
        }
        RecentOrder.Remove(node);
        RecentOrder.AddFirst(node);
        return node.Value.Bitmap;
    }

    /// <summary>Forgets every recent picture: Settings → Storage cleared the cache, so images load again from the servers.</summary>
    public static void ForgetRecent()
    {
        Recent.Clear();
        RecentOrder.Clear();
    }

    private static void Remember(ImageRequest request, BitmapImage bitmap)
    {
        if (Recent.TryGetValue(request, out var old))
        {
            RecentOrder.Remove(old);
        }
        Recent[request] = RecentOrder.AddFirst((request, bitmap));
        while (RecentOrder.Count > RecentCapacity && RecentOrder.Last is { } last)
        {
            RecentOrder.RemoveLast();
            Recent.Remove(last.Value.Request);
        }
    }

    /// <summary>
    /// The image for <paramref name="request"/>, or null if a later load or <see cref="Recycle"/> cancelled this one. Width and height
    /// are physical pixels: the box's size in view pixels times <c>XamlRoot.RasterizationScale</c>.
    /// </summary>
    /// <exception cref="ImageLoadException">The image could not be loaded; the tile keeps its placeholder.</exception>
    public async Task<BitmapImage?> LoadAsync(ImageRequest request)
    {
        Recycle();
        if (TryRecent(request) is { } recent)
        {
            return recent;
        }
        var load = new CancellationTokenSource();
        _load = load;
        try
        {
            var bytes = await core.LoadImageAsync(request, load.Token);
            if (load.IsCancellationRequested)
            {
                return null;
            }
            var bitmap = new BitmapImage { DecodePixelWidth = request.Width };
            using var stream = new MemoryStream(bytes).AsRandomAccessStream();
            await bitmap.SetSourceAsync(stream);
            Remember(request, bitmap);
            return load.IsCancellationRequested ? null : bitmap;
        }
        catch (OperationCanceledException) when (load.IsCancellationRequested)
        {
            return null;
        }
        finally
        {
            if (ReferenceEquals(_load, load))
            {
                _load = null;
            }
            load.Dispose();
        }
    }

    /// <summary>The tile was recycled or left the view: stop its load.</summary>
    public void Recycle()
    {
        _load?.Cancel();
        _load = null;
    }

    public void Dispose() => Recycle();
}
