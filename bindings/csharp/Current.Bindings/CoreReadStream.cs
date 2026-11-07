using Ffi = uniffi.current_ffi;

namespace Current.Bindings;

/// <summary>
/// The core's stream reader for one file (ADR 0026): the platform engine reads a direct-play file through the core instead of a
/// URL. <see cref="OpenAsync"/> makes the <see cref="Stream"/> to hand Windows' player (<c>MediaSource.CreateFromStream</c> over
/// <c>AsRandomAccessStream()</c>). Until R3 a load has one only when the player's switch is on
/// (<see cref="PlayerSession.SetStreamReader"/>).
/// </summary>
public sealed class CoreFileReader
{
    private readonly Ffi.StreamReader _reader;

    internal CoreFileReader(Ffi.StreamReader reader) => _reader = reader;

    /// <summary>Opens the file (the core's first request for it gives its size and type) and returns a stream over it. Every call
    /// opens the same file: later calls answer at once.</summary>
    /// <exception cref="OperationCanceledException"><paramref name="ct"/> was cancelled, or the playback moved on.</exception>
    /// <exception cref="IOException">The server refused or failed (the player shows why), or the reader was closed.</exception>
    public async Task<CoreReadStream> OpenAsync(CancellationToken ct = default)
    {
        var info = await StreamCall.RunAsync(token => _reader.Open(token), ct).ConfigureAwait(false);
        return new CoreReadStream(_reader, info.Size, info.ContentType);
    }

    public override string ToString() => "CoreFileReader";
}

/// <summary>
/// A read-only, seekable <see cref="Stream"/> over the core's stream reader. Reads wait while the core has not fetched the bytes yet
/// or while they are further ahead of playback than the core lets a player read (150 MiB, as libmpv). One read at a time, as for any
/// <see cref="Stream"/>. Disposing it closes the reader for the platform; the file's requests end with the playback.
/// </summary>
public sealed class CoreReadStream : Stream
{
    private readonly Ffi.StreamReader _reader;
    private long _position;
    private int _disposed;

    internal CoreReadStream(Ffi.StreamReader reader, ulong size, string contentType)
    {
        _reader = reader;
        Length = size > long.MaxValue ? long.MaxValue : (long)size;
        ContentType = contentType;
    }

    /// <summary>The file's content type as the server named it (<c>video/x-matroska</c>), for <c>MediaSource.CreateFromStream</c>.</summary>
    public string ContentType { get; }

    public override bool CanRead => Volatile.Read(ref _disposed) == 0;

    public override bool CanSeek => Volatile.Read(ref _disposed) == 0;

    public override bool CanWrite => false;

    public override long Length { get; }

    public override long Position
    {
        get => Interlocked.Read(ref _position);
        set => Seek(value, SeekOrigin.Begin);
    }

    public override long Seek(long offset, SeekOrigin origin)
    {
        ObjectDisposedException.ThrowIf(Volatile.Read(ref _disposed) != 0, this);
        var target = origin switch
        {
            SeekOrigin.Begin => offset,
            SeekOrigin.Current => Position + offset,
            SeekOrigin.End => Length + offset,
            _ => throw new ArgumentOutOfRangeException(nameof(origin)),
        };
        if (target < 0)
        {
            throw new IOException("a position before the stream's start");
        }
        Interlocked.Exchange(ref _position, target);
        return target;
    }

    /// <exception cref="OperationCanceledException"><paramref name="cancellationToken"/> was cancelled, or the playback moved on
    /// (stop, another item or route, the player closed).</exception>
    /// <exception cref="IOException">The server refused or failed, or the stream was closed.</exception>
    public override async ValueTask<int> ReadAsync(Memory<byte> buffer, CancellationToken cancellationToken = default)
    {
        ObjectDisposedException.ThrowIf(Volatile.Read(ref _disposed) != 0, this);
        var offset = Position;
        if (buffer.IsEmpty || offset >= Length)
        {
            return 0;
        }
        var bytes = await StreamCall.RunAsync(
            token => _reader.Read((ulong)offset, (uint)buffer.Length, token), cancellationToken).ConfigureAwait(false);
        var count = Math.Min(bytes.Length, buffer.Length);
        bytes.AsSpan(0, count).CopyTo(buffer.Span);
        Interlocked.Exchange(ref _position, offset + count);
        return count;
    }

    public override Task<int> ReadAsync(byte[] buffer, int offset, int count, CancellationToken cancellationToken) =>
        ReadAsync(buffer.AsMemory(offset, count), cancellationToken).AsTask();

    /// <summary>Blocks on <see cref="ReadAsync(Memory{byte}, CancellationToken)"/>: never call it on the UI thread.</summary>
    public override int Read(byte[] buffer, int offset, int count) =>
        ReadAsync(buffer.AsMemory(offset, count)).AsTask().GetAwaiter().GetResult();

    public override void Flush()
    {
    }

    public override void SetLength(long value) => throw new NotSupportedException();

    public override void Write(byte[] buffer, int offset, int count) => throw new NotSupportedException();

    protected override void Dispose(bool disposing)
    {
        if (disposing && Interlocked.Exchange(ref _disposed, 1) == 0)
        {
            _reader.Close();
        }
        base.Dispose(disposing);
    }
}

/// <summary>Runs a stream reader call with a fresh <c>CancelToken</c> that the caller's token cancels (ADR 0004), and maps its
/// errors: <c>Cancelled</c> to <see cref="OperationCanceledException"/>, the others to <see cref="IOException"/>.</summary>
internal static class StreamCall
{
    public static async Task<T> RunAsync<T>(Func<Ffi.CancelToken, Task<T>> call, CancellationToken ct)
    {
        using var token = new Ffi.CancelToken();
        using var registration = ct.Register(token.Cancel);
        try
        {
            return await call(token).ConfigureAwait(false);
        }
        catch (Ffi.StreamReadException.Cancelled)
        {
            throw ct.IsCancellationRequested
                ? new OperationCanceledException(ct)
                : new OperationCanceledException("the playback moved on");
        }
        catch (Ffi.StreamReadException.Closed)
        {
            throw new IOException("the stream was closed");
        }
        catch (Ffi.StreamReadException)
        {
            throw new IOException("the stream could not be read");
        }
    }
}
