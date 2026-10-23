using System.Collections.ObjectModel;
using System.ComponentModel;
using System.Diagnostics;
using Current.Bindings;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Navigation;
using Windows.System;

namespace Current.Shell.Pages;

/// <summary>
/// Adding a server (ADR 0012, ADR 0016): probe a typed address or pick a server found on the network, confirm plain http when the core
/// asks, then go on to sign in. The page renders the flows' states and sends the user's actions; the core decides. It owns one
/// AddServerFlow and one DiscoveryFlow from OnNavigatedTo to OnNavigatedFrom.
/// </summary>
public sealed partial class AddServerPage : Page
{
    private readonly ObservableCollection<DiscoveredServer> _found = [];
    private AddServerFlow? _flow;
    private DiscoveryFlow? _discovery;
    private ValueSubscription<AddServerState>? _state;
    private ValueSubscription<DiscoveryState>? _search;
    private CancellationTokenSource? _probe;
    private CancellationTokenSource? _searching;
    private AddServerState? _shown;
    private bool _confirming;
    private bool _leaving;

    public AddServerPage()
    {
        InitializeComponent();
        FoundList.ItemsSource = _found;
        Loaded += (_, _) => AddressBox.Focus(FocusState.Programmatic);
    }

    protected override void OnNavigatedTo(NavigationEventArgs e)
    {
        base.OnNavigatedTo(e);
        if (App.Core is not { } core)
        {
            return;
        }
        var ui = new DispatcherQueueUi(DispatcherQueue);
        _flow = core.AddServer();
        _discovery = core.DiscoverServers();
        _state = _flow.ObserveState(ui);
        _state.PropertyChanged += OnStateChanged;
        _search = _discovery.ObserveState(ui);
        _search.PropertyChanged += OnSearchChanged;
        Render(_state.Value);
        RenderSearch(_search.Value);
        // Searching starts by itself (manager, 2026-10-23).
        StartSearch();
    }

    protected override void OnNavigatedFrom(NavigationEventArgs e)
    {
        base.OnNavigatedFrom(e);
        _leaving = true;
        _probe?.Cancel();
        _searching?.Cancel();
        if (_state is not null)
        {
            _state.PropertyChanged -= OnStateChanged;
            _state.Dispose();
        }
        if (_search is not null)
        {
            _search.PropertyChanged -= OnSearchChanged;
            _search.Dispose();
        }
        _flow?.Dispose();
        _discovery?.Dispose();
        _state = null;
        _search = null;
        _flow = null;
        _discovery = null;
    }

    private void OnStateChanged(object? sender, PropertyChangedEventArgs e)
    {
        if (_state is not null)
        {
            Render(_state.Value);
        }
    }

    private void OnSearchChanged(object? sender, PropertyChangedEventArgs e)
    {
        if (_search is not null)
        {
            RenderSearch(_search.Value);
        }
    }

    private void Render(AddServerState state)
    {
        var before = _shown;
        _shown = state;
        var probing = state is AddServerState.Probing;
        // Read-only rather than disabled, so the box keeps focus while the probe runs.
        AddressBox.IsReadOnly = probing;
        ConnectButton.IsEnabled = !probing;
        CancelButton.Visibility = probing ? Visibility.Visible : Visibility.Collapsed;
        ProbingPanel.Visibility = probing ? Visibility.Visible : Visibility.Collapsed;
        ProbingRing.IsActive = probing;
        FoundList.IsEnabled = !probing;
        if (probing && before is not AddServerState.Probing)
        {
            Announce.Shown(ProbingText);
        }
        else if (before is AddServerState.Probing && state is AddServerState.Idle or AddServerState.Failed)
        {
            // Cancel had focus and is now hidden: back to the address.
            DispatcherQueue.TryEnqueue(() => AddressBox.Focus(FocusState.Programmatic));
        }

        switch (state)
        {
            case AddServerState.Failed failed:
                ShowError(ProblemText.Probe(failed.Problem));
                break;
            case AddServerState.ConfirmPlainHttp confirm:
                HideError();
                _ = ConfirmPlainHttpAsync(confirm.Server);
                break;
            case AddServerState.Ready ready:
                HideError();
                // Leave the page after this delivery has finished, not inside it.
                DispatcherQueue.TryEnqueue(() => GoToSignIn(ready.Server));
                break;
            default:
                HideError();
                break;
        }
    }

    private void RenderSearch(DiscoveryState state)
    {
        var (found, searching) = state switch
        {
            DiscoveryState.Searching s => (s.Found, true),
            DiscoveryState.Done d => (d.Found, false),
            _ => ((IReadOnlyList<DiscoveredServer>)[], false),
        };
        SyncFound(found);
        SearchingBar.Visibility = searching ? Visibility.Visible : Visibility.Collapsed;
        SearchAgainButton.Visibility = state is DiscoveryState.Done or DiscoveryState.Failed ? Visibility.Visible : Visibility.Collapsed;
        var status = state switch
        {
            DiscoveryState.Searching { Found.Count: 0 } => "Searching…",
            DiscoveryState.Done { Found.Count: 0 } => "No servers found on this network. Enter the address instead.",
            DiscoveryState.Failed failed => ProblemText.Discovery(failed.Problem),
            _ => "",
        };
        SearchStatus.Text = status;
        SearchStatus.Visibility = status.Length > 0 ? Visibility.Visible : Visibility.Collapsed;
    }

    /// <summary>Servers are listed in the order they answered: append new ones, so focus and the list stay put while searching.</summary>
    private void SyncFound(IReadOnlyList<DiscoveredServer> found)
    {
        var prefix = _found.Count <= found.Count && _found.Select(s => s.Id).SequenceEqual(found.Take(_found.Count).Select(s => s.Id));
        if (!prefix)
        {
            _found.Clear();
        }
        foreach (var server in found.Skip(_found.Count))
        {
            _found.Add(server);
        }
    }

    private void ShowError(string message)
    {
        // Close and open again, so Narrator reads a changed message (InfoBar page, learn.microsoft.com, read 2026-10-23).
        ProbeError.IsOpen = false;
        ProbeError.Message = message;
        ProbeError.IsOpen = true;
    }

    private void HideError() => ProbeError.IsOpen = false;

    private void OnAddressChanged(object sender, TextChangedEventArgs e)
    {
        if (_state?.Value is AddServerState.Failed)
        {
            HideError();
        }
    }

    private void OnAddressKeyDown(object sender, KeyRoutedEventArgs e)
    {
        if (e.Key == VirtualKey.Enter)
        {
            e.Handled = true;
            Connect();
        }
    }

    private void OnConnect(object sender, RoutedEventArgs e) => Connect();

    // async void, as event handlers are: it catches everything itself.
    private async void Connect()
    {
        if (_flow is null || _probe is not null || _state?.Value is AddServerState.Probing)
        {
            return;
        }
        using var probe = new CancellationTokenSource();
        _probe = probe;
        try
        {
            await _flow.ProbeAsync(AddressBox.Text, probe.Token);
        }
        catch (OperationCanceledException)
        {
            // Cancelled by the user or by leaving the page: the flow is back to Idle.
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: probing failed: {error.GetType().Name}");
            if (!_leaving)
            {
                ShowError(ProblemText.Unexpected);
            }
        }
        finally
        {
            _probe = null;
        }
    }

    private void OnCancel(object sender, RoutedEventArgs e) => CancelProbe();

    private void OnEscape(KeyboardAccelerator sender, KeyboardAcceleratorInvokedEventArgs args) => args.Handled = CancelProbe();

    private bool CancelProbe()
    {
        if (_probe is null)
        {
            return false;
        }
        _probe.Cancel();
        return true;
    }

    /// <summary>Names each found server for Narrator, which would otherwise read the item's type.</summary>
    private void OnFoundContainerChanging(ListViewBase sender, ContainerContentChangingEventArgs args)
    {
        if (args.Item is DiscoveredServer server)
        {
            AutomationProperties.SetName(args.ItemContainer, server.Name);
        }
    }

    private void OnPickServer(object sender, ItemClickEventArgs e)
    {
        if (_flow is null || _discovery is null || e.ClickedItem is not DiscoveredServer server)
        {
            return;
        }
        _probe?.Cancel();
        if (!_flow.Pick(_discovery, server.Id))
        {
            // The search no longer lists it: a new search has started since.
            ShowError("That server is no longer listed. Pick it again or enter its address.");
        }
    }

    private void OnSearchAgain(object sender, RoutedEventArgs e) => StartSearch();

    // async void, as event handlers are: it catches everything itself.
    private async void StartSearch()
    {
        if (_discovery is null || _searching is not null)
        {
            return;
        }
        using var searching = new CancellationTokenSource();
        _searching = searching;
        try
        {
            await _discovery.SearchAsync(searching.Token);
        }
        catch (OperationCanceledException)
        {
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: searching for servers failed: {error.GetType().Name}");
        }
        finally
        {
            _searching = null;
        }
    }

    /// <summary>
    /// A server outside the local network answered over plain http: a native confirmation (Dialogs page, learn.microsoft.com, ms.date
    /// 2026-07-15, read 2026-10-23). The close button is the safe action and also answers Esc; it is the default button, so Enter does
    /// the safe thing too.
    /// </summary>
    private async Task ConfirmPlainHttpAsync(NewServerInfo server)
    {
        if (_confirming || XamlRoot is null)
        {
            return;
        }
        _confirming = true;
        try
        {
            var dialog = new ContentDialog
            {
                XamlRoot = XamlRoot,
                Title = "Connect without encryption?",
                Content = $"{server.Name} answered over http. Your password and everything you watch would cross the internet "
                    + "unencrypted. Use https if the server offers it.",
                PrimaryButtonText = "Connect anyway",
                CloseButtonText = "Cancel",
                DefaultButton = ContentDialogButton.Close,
            };
            var result = await dialog.ShowAsync();
            if (_flow is null)
            {
                return;
            }
            if (result == ContentDialogResult.Primary)
            {
                _flow.ConfirmPlainHttp();
            }
            else
            {
                _flow.Reset();
                AddressBox.Focus(FocusState.Programmatic);
            }
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: the plain-http confirmation failed: {error.GetType().Name}");
        }
        finally
        {
            _confirming = false;
        }
    }

    private void GoToSignIn(NewServerInfo server)
    {
        if (_flow is null || _leaving || _state?.Value is not AddServerState.Ready)
        {
            return;
        }
        SignInFlow flow;
        try
        {
            flow = _flow.SignIn();
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: starting the sign-in failed: {error.GetType().Name}");
            ShowError(ProblemText.Unexpected);
            return;
        }
        if (!Frame.Navigate(typeof(SignInPage), new SignInRequest(flow, server.Name, server.HttpsFailedTls, null)))
        {
            flow.Dispose();
        }
    }
}
