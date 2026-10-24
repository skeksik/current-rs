using System.Diagnostics;
using Current.Bindings;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.Web.WebView2.Core;

namespace Current.Shell.Gateway;

/// <summary>
/// The sign-in window of a web sign-in gateway (ADR 0017, "Mechanism 2"): its own window with WebView2 on a throw-away profile
/// (<see cref="SignInProfiles"/>). It starts at the address the core gives, shows the origin of every page, and after each navigation
/// that lands on the server's origin hands the cookies for the server's address over (HttpOnly included). The owner closes it when the
/// core moves on (<see cref="CloseFromApp"/>); the user closing it cancels (<see cref="Cancelled"/>). No script bridge, no downloads,
/// pop-ups in the same window, and nothing filled in by the app.
/// </summary>
public sealed partial class SignInWindow : Window
{
    /// <summary>The one host name the app knows, and only for a message (ADR 0017, "Google").</summary>
    private const string GoogleSignInHost = "accounts.google.com";

    /// <summary>
    /// How soon after "Back to the sign-in page" a new turn to Google means the gateway forwards straight there (Google its only
    /// method, as the owner's run had it, 2026-10-24): Back is then of no use and goes away.
    /// </summary>
    private static readonly TimeSpan BackLoop = TimeSpan.FromSeconds(5);

    /// <summary>The open sign-in windows: closed with the main window, which owns the core they talk to.</summary>
    private static readonly List<SignInWindow> Open = [];

    private readonly Uri _start;
    private readonly string _origin;
    private readonly Func<IReadOnlyList<GatewayCookie>, Task> _handOver;
    private readonly TaskCompletionSource<bool> _firstPage = new(TaskCreationOptions.RunContinuationsAsynchronously);
    private string? _folder;
    private uint _browserProcess;
    private bool _closingFromApp;
    private bool _closed;
    private DateTime? _backAt;

    /// <param name="url">Where the window starts: the server's address the core probed.</param>
    /// <param name="origin">The server's origin as the core writes it (<c>scheme://host[:port]</c>): the title names it, and pages that
    /// land on it hand their cookies over.</param>
    /// <param name="handOver">Hands the cookies to the core's flow.</param>
    internal SignInWindow(string url, string origin, Func<IReadOnlyList<GatewayCookie>, Task> handOver)
    {
        InitializeComponent();
        _start = new Uri(url);
        _origin = origin;
        _handOver = handOver;
        // "Sign in to media.example.com" (ADR 0017): the origin without its scheme, which the origin bar shows.
        Title = $"Sign in to {WithoutScheme(origin)}";
        ShowOrigin(_start);
        Closed += OnClosed;
        Open.Add(this);
        _ = StartAsync();
    }

    /// <summary>The user closed the window: the sign-in is cancelled. Not raised by <see cref="CloseFromApp"/>.</summary>
    public event EventHandler? Cancelled;

    /// <summary>Completes when the first page has loaded (true), or the web view could not start (false). For the smoke run.</summary>
    internal Task<bool> FirstPage => _firstPage.Task;

    /// <summary>What the origin bar shows, and whether with the lock. For the smoke run.</summary>
    internal (string Origin, bool Locked) OriginShown => (OriginText.Text, OriginIcon.Glyph == LockGlyph);

    /// <summary>Navigates the page, for the smoke run (a person clicks a link instead).</summary>
    internal void NavigateForTest(string url) => Web.CoreWebView2?.Navigate(url);

    /// <summary>Closes every sign-in window, without <see cref="Cancelled"/>: the main window is closing.</summary>
    internal static void CloseAll()
    {
        foreach (var window in Open.ToList())
        {
            window.CloseFromApp();
        }
    }

    /// <summary>Closes the window because the core moved on: no <see cref="Cancelled"/>.</summary>
    public void CloseFromApp()
    {
        if (_closed)
        {
            return;
        }
        _closingFromApp = true;
        Close();
    }

    private const string LockGlyph = "";
    private const string WarningGlyph = "";

    private async Task StartAsync()
    {
        try
        {
            _folder = SignInProfiles.Create();
            var environment = await CoreWebView2Environment.CreateWithOptionsAsync(null, _folder, new CoreWebView2EnvironmentOptions());
            await Web.EnsureCoreWebView2Async(environment);
            if (_closed)
            {
                return;
            }
            var web = Web.CoreWebView2;
            _browserProcess = web.BrowserProcessId;
            var settings = web.Settings;
            // No script bridge to the app (ADR 0017): no web messages, no host objects. No developer tools, and nothing autofilled
            // or offered to be saved: the profile is thrown away anyway.
            settings.IsWebMessageEnabled = false;
            settings.AreHostObjectsAllowed = false;
            settings.AreDevToolsEnabled = false;
            settings.IsPasswordAutosaveEnabled = false;
            settings.IsGeneralAutofillEnabled = false;
            web.NavigationStarting += OnNavigationStarting;
            web.SourceChanged += (_, _) => ShowOrigin(SourceOf(web));
            web.NavigationCompleted += OnNavigationCompleted;
            web.NewWindowRequested += OnNewWindowRequested;
            web.DownloadStarting += OnDownloadStarting;
            web.ServerCertificateErrorDetected += OnServerCertificateError;
            web.Navigate(_start.AbsoluteUri);
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: the sign-in window's web view did not start: {error.GetType().Name}");
            Web.Visibility = Visibility.Collapsed;
            ShowNotice(
                InfoBarSeverity.Error,
                WebViewRuntime.IsInstalled()
                    ? "The sign-in page couldn't be opened. Close this window and try again."
                    : WebViewRuntime.Missing);
            _firstPage.TrySetResult(false);
        }
    }

    /// <summary>Google's sign-in is not opened: the window explains instead (ADR 0017, "Google").</summary>
    private void OnNavigationStarting(CoreWebView2 sender, CoreWebView2NavigationStartingEventArgs args)
    {
        if (Uri.TryCreate(args.Uri, UriKind.Absolute, out var uri)
            && string.Equals(uri.Host, GoogleSignInHost, StringComparison.OrdinalIgnoreCase))
        {
            args.Cancel = true;
            ShowGoogle(true);
            return;
        }
        ShowGoogle(false);
        Notice.IsOpen = false;
    }

    // async void, as event handlers are: it catches everything itself.
    private async void OnNavigationCompleted(CoreWebView2 sender, CoreWebView2NavigationCompletedEventArgs args)
    {
        var source = SourceOf(sender);
        ShowOrigin(source);
        _firstPage.TrySetResult(true);
        if (source is null || !string.Equals(OriginOf(source), _origin, StringComparison.OrdinalIgnoreCase))
        {
            return;
        }
        try
        {
            // Every cookie the web view would send to the server's address, HttpOnly included; the core keeps those that apply.
            var cookies = await sender.CookieManager.GetCookiesAsync(_start.AbsoluteUri);
            if (_closed)
            {
                return;
            }
            var handed = cookies
                .Select(cookie => new GatewayCookie(
                    cookie.Name,
                    cookie.Value,
                    cookie.Domain,
                    cookie.Path,
                    cookie.IsSecure,
                    cookie.IsHttpOnly,
                    cookie.IsSession ? null : DateTimeOffset.FromUnixTimeMilliseconds((long)(cookie.Expires * 1000))))
                .ToList();
            if (handed.Count > 0)
            {
                await _handOver(handed);
            }
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: handing the sign-in page's cookies over failed: {error.GetType().Name}");
        }
    }

    /// <summary>Pop-ups open in this window (ADR 0017).</summary>
    private static void OnNewWindowRequested(CoreWebView2 sender, CoreWebView2NewWindowRequestedEventArgs args)
    {
        args.Handled = true;
        sender.Navigate(args.Uri);
    }

    /// <summary>No downloads (ADR 0017).</summary>
    private void OnDownloadStarting(CoreWebView2 sender, CoreWebView2DownloadStartingEventArgs args)
    {
        args.Cancel = true;
        args.Handled = true;
        ShowNotice(InfoBarSeverity.Informational, "Downloads aren't available in the sign-in window.");
    }

    /// <summary>
    /// A certificate this PC does not trust is refused, never offered to proceed: so every https page the window shows is verified,
    /// and the lock means what it says.
    /// </summary>
    private void OnServerCertificateError(CoreWebView2 sender, CoreWebView2ServerCertificateErrorDetectedEventArgs args)
    {
        args.Action = CoreWebView2ServerCertificateErrorAction.Cancel;
        ShowNotice(
            InfoBarSeverity.Error,
            "The page's secure connection failed: its certificate isn't trusted by this PC, so it wasn't opened.");
    }

    private void OnGoogleBack(object sender, RoutedEventArgs e)
    {
        _backAt = DateTime.UtcNow;
        ShowGoogle(false);
        if (Web.CoreWebView2 is { } web)
        {
            if (web.CanGoBack)
            {
                web.GoBack();
            }
            else
            {
                web.Navigate(_start.AbsoluteUri);
            }
        }
    }

    /// <summary>"Close": the user cancels, as with the window's own close button.</summary>
    private void OnGoogleClose(object sender, RoutedEventArgs e) => Close();

    private void ShowGoogle(bool shown)
    {
        if (shown == (GooglePanel.Visibility == Visibility.Visible))
        {
            return;
        }
        // Back led straight to Google again: the gateway has no other method to go back to.
        var looped = shown && _backAt is { } backAt && DateTime.UtcNow - backAt < BackLoop;
        _backAt = null;
        GooglePanel.Visibility = shown ? Visibility.Visible : Visibility.Collapsed;
        Web.Visibility = shown ? Visibility.Collapsed : Visibility.Visible;
        // Set when shown, so the live region is read each time (Announce.Shown).
        GoogleText.Text = !shown ? ""
            : looped ? $"{ProblemText.GoogleRefusesWebViews} This gateway sends straight back to Google, so it has no other "
                + "sign-in method here."
            : ProblemText.GoogleRefusesWebViews;
        if (looped)
        {
            GoogleBackButton.Visibility = Visibility.Collapsed;
        }
        if (shown)
        {
            Announce.Shown(GoogleText);
            var focus = GoogleBackButton.Visibility == Visibility.Visible ? GoogleBackButton : GoogleCloseButton;
            DispatcherQueue.TryEnqueue(() => focus.Focus(FocusState.Programmatic));
        }
    }

    private void ShowOrigin(Uri? page)
    {
        if (page is null || !page.IsAbsoluteUri || page.Scheme is not ("http" or "https"))
        {
            return;
        }
        var origin = OriginOf(page);
        var secure = page.Scheme == "https";
        OriginText.Text = origin;
        OriginIcon.Glyph = secure ? LockGlyph : WarningGlyph;
        AutomationProperties.SetName(
            OriginBar, secure ? $"Page address {origin}, secure connection" : $"Page address {origin}, not secure");
    }

    private void ShowNotice(InfoBarSeverity severity, string message)
    {
        // Closed and opened again, so Narrator reads a changed message (InfoBar page, learn.microsoft.com, read 2026-10-23).
        Notice.IsOpen = false;
        Notice.Severity = severity;
        Notice.Message = message;
        Notice.IsOpen = true;
    }

    private void OnClosed(object sender, WindowEventArgs args)
    {
        _closed = true;
        Open.Remove(this);
        _firstPage.TrySetResult(false);
        var browser = _browserProcess;
        var folder = _folder;
        try
        {
            Web.Close();
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: closing the sign-in web view failed: {error.GetType().Name}");
        }
        if (folder is not null)
        {
            _ = SignInProfiles.DeleteAsync(folder, browser);
        }
        if (!_closingFromApp)
        {
            Cancelled?.Invoke(this, EventArgs.Empty);
        }
    }

    private static Uri? SourceOf(CoreWebView2 web) => Uri.TryCreate(web.Source, UriKind.Absolute, out var uri) ? uri : null;

    /// <summary><c>scheme://host[:port]</c>, the port only when it is not the scheme's default: as the core writes an origin.</summary>
    internal static string OriginOf(Uri page) => page.GetLeftPart(UriPartial.Authority);

    private static string WithoutScheme(string origin)
    {
        var at = origin.IndexOf("://", StringComparison.Ordinal);
        return at < 0 ? origin : origin[(at + 3)..];
    }
}
