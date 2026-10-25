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
    /// How long a picture has to arrive before its BlurHash is shown in its place. A picture the core has in memory or on disk
    /// arrives within it and shows straight away, with no blurred step before it; the BlurHash is for pictures still coming from the
    /// server (the owner's run, 2026-10-25: pages blinked blank, then blurred, then sharp). 100 ms is about six frames at 60 Hz, a
    /// first value to tune on screen, not from a guideline.
    /// </summary>
    public static readonly TimeSpan PlaceholderDelay = TimeSpan.FromMilliseconds(100);

    private CancellationTokenSource? _load;

    /// <summary>
    /// The image for <paramref name="request"/>, or null if a later load or <see cref="Recycle"/> cancelled this one. Width and height
    /// are physical pixels: the box's size in view pixels times <c>XamlRoot.RasterizationScale</c>.
    /// </summary>
    /// <exception cref="ImageLoadException">The image could not be loaded; the tile keeps its placeholder.</exception>
    public async Task<BitmapImage?> LoadAsync(ImageRequest request)
    {
        Recycle();
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
