using System.Collections.ObjectModel;
using System.ComponentModel;
using System.Diagnostics;
using System.Runtime.CompilerServices;
using Current.Bindings;
using Microsoft.UI.Input;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;
using Microsoft.UI.Xaml.Input;
using VirtualKey = Windows.System.VirtualKey;

namespace Current.Shell.Browsing;

/// <summary>
/// One group the user arranges (ADR 0024): its rows in the core's order, each moved by dragging, by its Move up and Move down
/// buttons or Alt+Up and Alt+Down, and shown or hidden by its switch. Every change goes to the core (<see cref="Move"/>,
/// <see cref="SetHidden"/>), which saves it and publishes the group again; <see cref="Show"/> then takes the new order in place, so
/// keyboard focus stays on the row.
/// </summary>
public sealed partial class ArrangeList : UserControl
{
    public ArrangeList()
    {
        InitializeComponent();
    }

    public ObservableCollection<ArrangeRow> Rows { get; } = [];

    /// <summary>The group's name for Narrator, as the heading above it reads.</summary>
    public string ListName
    {
        get => Microsoft.UI.Xaml.Automation.AutomationProperties.GetName(List);
        set => Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(List, value);
    }

    /// <summary>Moves the row with this key to this index (of the group as shown, hidden rows included).</summary>
    public Func<string, int, Task>? Move { get; set; }

    /// <summary>Hides (true) or shows the row with this key.</summary>
    public Func<string, bool, Task>? SetHidden { get; set; }

    /// <summary>The rows as the core now arranges them: kept rows are updated and moved in place, new ones added, gone ones removed.</summary>
    public void Show(IReadOnlyList<ArrangeRow> rows)
    {
        for (var i = 0; i < rows.Count; i++)
        {
            var row = rows[i];
            row.CanMoveUp = i > 0;
            row.CanMoveDown = i < rows.Count - 1;
            var at = -1;
            for (var j = i; j < Rows.Count; j++)
            {
                if (Rows[j].Key == row.Key)
                {
                    at = j;
                    break;
                }
            }
            if (at < 0)
            {
                Rows.Insert(i, row);
                continue;
            }
            if (at != i)
            {
                Rows.Move(at, i);
            }
            Rows[i].Update(row);
        }
        while (Rows.Count > rows.Count)
        {
            Rows.RemoveAt(Rows.Count - 1);
        }
    }

    private static ArrangeRow? RowOf(object sender) => (sender as FrameworkElement)?.Tag as ArrangeRow;

    private void OnMoveUp(object sender, RoutedEventArgs e)
    {
        if (RowOf(sender) is { } row)
        {
            MoveBy(row, -1);
        }
    }

    private void OnMoveDown(object sender, RoutedEventArgs e)
    {
        if (RowOf(sender) is { } row)
        {
            MoveBy(row, 1);
        }
    }

    private void MoveBy(ArrangeRow row, int by)
    {
        var at = Rows.IndexOf(row);
        var to = at + by;
        if (at < 0 || to < 0 || to >= Rows.Count)
        {
            return;
        }
        Run(Move?.Invoke(row.Key, to));
        Announce.Say(this, $"{row.Name}, moved to position {to + 1} of {Rows.Count}", "arrange");
    }

    /// <summary>A drag ended: the list already shows the row where it was dropped, which the core is told.</summary>
    private void OnDragItemsCompleted(ListViewBase sender, DragItemsCompletedEventArgs args)
    {
        if (args.DropResult != Windows.ApplicationModel.DataTransfer.DataPackageOperation.Move)
        {
            return;
        }
        foreach (var row in args.Items.OfType<ArrangeRow>())
        {
            Run(Move?.Invoke(row.Key, Rows.IndexOf(row)));
        }
    }

    /// <summary>Alt+Up and Alt+Down move the row that has focus, or the row of the button that has it.</summary>
    private void OnListKeyDown(object sender, KeyRoutedEventArgs e)
    {
        if (e.Key is not (VirtualKey.Up or VirtualKey.Down)
            || !InputKeyboardSource.GetKeyStateForCurrentThread(VirtualKey.Menu).HasFlag(Windows.UI.Core.CoreVirtualKeyStates.Down)
            || (e.OriginalSource as FrameworkElement)?.DataContext is not ArrangeRow row)
        {
            return;
        }
        e.Handled = true;
        MoveBy(row, e.Key == VirtualKey.Up ? -1 : 1);
    }

    private void OnSwitchToggled(object sender, RoutedEventArgs e)
    {
        // Set from the core (IsOn follows IsShown): nothing to tell it.
        if (sender is ToggleSwitch toggle && RowOf(sender) is { } row && toggle.IsOn != row.IsShown)
        {
            Run(SetHidden?.Invoke(row.Key, !toggle.IsOn));
        }
    }

    private void OnShowClick(object sender, RoutedEventArgs e)
    {
        if (sender is ToggleButton toggle && RowOf(sender) is { } row)
        {
            Run(SetHidden?.Invoke(row.Key, toggle.IsChecked != true));
        }
    }

    // async void, as event handlers are: it catches everything itself. A failed save leaves the group as the core had it, which it
    // publishes again; the owner of the list shows why (App.ShowNotice).
    private static async void Run(Task? change)
    {
        if (change is null)
        {
            return;
        }
        try
        {
            await change;
        }
        catch (CoreStoreException)
        {
            App.ShowNotice(InfoBarSeverity.Error, "Couldn't save the change", "Current couldn't write its settings file.");
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: an arrangement change failed: {error.GetType().Name}");
        }
    }
}

/// <summary>One row of an <see cref="ArrangeList"/>: an item of a group, keyed as the core keys it.</summary>
public sealed class ArrangeRow(string key, string name, bool isShown, string glyph = "", string note = "", bool compact = false)
    : INotifyPropertyChanged
{
    public event PropertyChangedEventHandler? PropertyChanged;

    public string Key { get; } = key;

    public string Name { get; private set; } = name;

    public string Glyph { get; } = glyph;

    public Visibility GlyphVisibility => Glyph.Length > 0 ? Visibility.Visible : Visibility.Collapsed;

    /// <summary>Why it starts hidden, under its name: "Hidden on the server".</summary>
    public string Note { get; private set; } = note;

    public Visibility NoteVisibility => Note.Length > 0 ? Visibility.Visible : Visibility.Collapsed;

    public bool IsShown { get; private set; } = isShown;

    /// <summary>A hidden row is dimmed, so the list still shows where it sits.</summary>
    public double RowOpacity => IsShown ? 1 : 0.6;

    private bool _canMoveUp;
    private bool _canMoveDown;

    public bool CanMoveUp
    {
        get => _canMoveUp;
        set => Set(ref _canMoveUp, value);
    }

    public bool CanMoveDown
    {
        get => _canMoveDown;
        set => Set(ref _canMoveDown, value);
    }

    /// <summary>The pane's narrow list shows an eye toggle button; Settings and the Home dialog a ToggleSwitch.</summary>
    public Visibility SwitchVisibility => compact ? Visibility.Collapsed : Visibility.Visible;

    public Visibility ButtonVisibility => compact ? Visibility.Visible : Visibility.Collapsed;

    public string MoveUpLabel => $"Move {Name} up";

    public string MoveDownLabel => $"Move {Name} down";

    public string ShowLabel => $"Show {Name}";

    /// <summary>Takes what the core now says of the same item.</summary>
    public void Update(ArrangeRow from)
    {
        if (Name != from.Name)
        {
            Name = from.Name;
            Changed(nameof(Name));
            Changed(nameof(MoveUpLabel));
            Changed(nameof(MoveDownLabel));
            Changed(nameof(ShowLabel));
        }
        if (Note != from.Note)
        {
            Note = from.Note;
            Changed(nameof(Note));
            Changed(nameof(NoteVisibility));
        }
        if (IsShown != from.IsShown)
        {
            IsShown = from.IsShown;
            Changed(nameof(IsShown));
            Changed(nameof(RowOpacity));
        }
        CanMoveUp = from.CanMoveUp;
        CanMoveDown = from.CanMoveDown;
    }

    private void Set(ref bool field, bool value, [CallerMemberName] string name = "")
    {
        if (field != value)
        {
            field = value;
            Changed(name);
        }
    }

    private void Changed(string name) => PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(name));
}
