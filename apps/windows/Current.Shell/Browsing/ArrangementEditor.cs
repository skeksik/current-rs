using Current.Bindings;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Automation.Peers;
using Microsoft.UI.Xaml.Controls;

namespace Current.Shell.Browsing;

/// <summary>
/// The arrangement editor of Settings' "Navigation pane" and "Home" expanders and of the Edit Home dialog (ADR 0024): for each
/// signed-in server (a heading per server when more than one is), one <see cref="ArrangeList"/> per group with its Reset button. It
/// follows the hub: a server signed in or out, its libraries or its arrangement changed. Dispose it when its page or dialog goes.
/// </summary>
internal sealed class ArrangementEditor : IDisposable
{
    private readonly BrowseHub _hub;
    private readonly bool _home;
    private readonly Dictionary<(ServerRef Server, NavGroup Group), (ArrangeList List, Button Reset)> _lists = [];

    /// <summary>The editor's content: put it in the expander or the dialog.</summary>
    public StackPanel Root { get; } = new() { Spacing = 4 };

    /// <summary>An editor of the pane's groups, or with <paramref name="home"/> of Home's rows.</summary>
    public ArrangementEditor(BrowseHub hub, bool home)
    {
        _hub = hub;
        _home = home;
        _hub.Changed += OnHubChanged;
        Show();
    }

    /// <summary>Whether a server is signed in, so there is something to arrange.</summary>
    public bool HasContent => Root.Children.Count > 0;

    private void OnHubChanged(object? sender, EventArgs e) => Show();

    private void Show()
    {
        var servers = _hub.Servers;
        var children = new List<UIElement>();
        var kept = new HashSet<(ServerRef, NavGroup)>();
        foreach (var server in servers)
        {
            if (servers.Count > 1)
            {
                children.Add(Heading(_hub.ServerName(server.Server), AutomationHeadingLevel.Level3));
            }
            foreach (var (group, name, rows, reset) in Groups(server))
            {
                if (rows.Count == 0)
                {
                    continue;
                }
                var key = (server.Server, group);
                kept.Add(key);
                if (!_lists.TryGetValue(key, out var entry))
                {
                    entry = (List(server, group, servers.Count > 1 ? $"{name}, {_hub.ServerName(server.Server)}" : name),
                        ResetButton(server, group, reset));
                    _lists[key] = entry;
                }
                entry.List.Show(rows);
                if (!_home || servers.Count > 1)
                {
                    children.Add(Heading(name, AutomationHeadingLevel.Level4));
                }
                children.Add(entry.List);
                children.Add(entry.Reset);
            }
        }
        foreach (var gone in _lists.Keys.Where(key => !kept.Contains(key)).ToList())
        {
            _lists.Remove(gone);
        }
        // Rebuilt only when what it holds changed, so the list or button that has focus stays in the tree.
        if (!Root.Children.SequenceEqual(children, new SameChild()))
        {
            Root.Children.Clear();
            foreach (var child in children)
            {
                Root.Children.Add(child);
            }
        }
    }

    /// <summary>The same children: the same lists and buttons, and headings with the same text.</summary>
    private sealed class SameChild : IEqualityComparer<UIElement>
    {
        public bool Equals(UIElement? x, UIElement? y) =>
            ReferenceEquals(x, y) || (x is TextBlock a && y is TextBlock b && a.Text == b.Text);

        public int GetHashCode(UIElement obj) =>
            obj is TextBlock text ? text.Text.GetHashCode(StringComparison.Ordinal) : obj.GetHashCode();
    }

    /// <summary>The groups of <paramref name="server"/> this editor shows, each with its name and its Reset button's text.</summary>
    private IEnumerable<(NavGroup Group, string Name, IReadOnlyList<ArrangeRow> Rows, string Reset)> Groups(ServerBrowse server)
    {
        if (_home)
        {
            yield return (NavGroup.HomeRows, "Home", PaneRows.HomeRows(server.Session.HomeLayout()), "Reset to default");
            yield break;
        }
        if (server.Pane is not { } pane)
        {
            yield break;
        }
        yield return (NavGroup.Tabs, "Pages", PaneRows.Tabs(pane), "Reset to default");
        yield return (NavGroup.Views, "Views", PaneRows.ViewItems(pane), "Reset to default");
        yield return (NavGroup.Libraries, "Libraries", PaneRows.Libraries(pane), "Reset to the server's order");
    }

    private ArrangeList List(ServerBrowse server, NavGroup group, string name)
    {
        var session = server.Session;
        return new ArrangeList
        {
            ListName = name,
            // Home's rows have no subscription of their own: read again after each change. The pane's follow the hub.
            Move = async (key, to) =>
            {
                await session.MoveAsync(group, key, to);
                ShowAgainIfHome();
            },
            SetHidden = async (key, hidden) =>
            {
                await session.SetHiddenAsync(group, key, hidden);
                ShowAgainIfHome();
            },
        };
    }

    private void ShowAgainIfHome()
    {
        if (_home)
        {
            Show();
        }
    }

    private Button ResetButton(ServerBrowse server, NavGroup group, string text)
    {
        var button = new Button { Content = text, Margin = new Thickness(0, 4, 0, 8) };
        button.Click += async (_, _) =>
        {
            try
            {
                await server.Session.ResetAsync(group);
            }
            catch (CoreStoreException)
            {
                App.ShowNotice(InfoBarSeverity.Error, "Couldn't save the change", "Current couldn't write its settings file.");
            }
            catch (Exception error)
            {
                System.Diagnostics.Trace.TraceError($"Current: a reset failed: {error.GetType().Name}");
            }
            ShowAgainIfHome();
        };
        return button;
    }

    private static TextBlock Heading(string text, AutomationHeadingLevel level)
    {
        var heading = new TextBlock
        {
            Text = text,
            Margin = new Thickness(0, 8, 0, 0),
            Style = (Style)Application.Current.Resources["BodyStrongTextBlockStyle"],
        };
        AutomationProperties.SetHeadingLevel(heading, level);
        return heading;
    }

    public void Dispose() => _hub.Changed -= OnHubChanged;
}
