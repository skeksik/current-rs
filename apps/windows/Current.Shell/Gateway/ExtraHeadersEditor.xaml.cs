using Current.Bindings;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;

namespace Current.Shell.Gateway;

/// <summary>
/// The extra headers editor: rows of a name and a value (a PasswordBox), "Add a header", and the core's reason when it refuses them.
/// It holds the values only in its boxes; <see cref="Read"/> hands them to the caller, which passes them to the core, and
/// <see cref="Clear"/> empties them. The caller decides where they go (add-server, or a saved server's gateway sign-in).
/// </summary>
public sealed partial class ExtraHeadersEditor : UserControl
{
    private readonly List<(TextBox Name, PasswordBox Value, Grid Row)> _rows = [];

    public ExtraHeadersEditor()
    {
        InitializeComponent();
        AddRow();
    }

    /// <summary>"Use these headers" was pressed (with <see cref="ShowsUseButton"/>).</summary>
    public event EventHandler? UseRequested;

    /// <summary>Whether the editor shows its own "Use these headers" button; a dialog uses its primary button instead.</summary>
    public bool ShowsUseButton
    {
        get => UseButton.Visibility == Visibility.Visible;
        set => UseButton.Visibility = value ? Visibility.Visible : Visibility.Collapsed;
    }

    /// <summary>Whether the user can change the rows and press the button, e.g. not while a check runs.</summary>
    public bool IsEditable
    {
        get => AddButton.IsEnabled;
        set
        {
            AddButton.IsEnabled = value;
            UseButton.IsEnabled = value;
            foreach (var (name, entry, row) in _rows)
            {
                name.IsReadOnly = !value;
                entry.IsEnabled = value;
                ((Button)row.Children[2]).IsEnabled = value;
            }
        }
    }

    /// <summary>The rows with a name, as typed (the core checks them). Empty rows are skipped.</summary>
    public IReadOnlyList<GatewayHeader> Read() => _rows
        .Where(row => row.Name.Text.Trim().Length > 0 || row.Value.Password.Length > 0)
        .Select(row => new GatewayHeader(row.Name.Text.Trim(), row.Value.Password))
        .ToList();

    /// <summary>Empties every value, and the names with them, leaving one empty row.</summary>
    public void Clear()
    {
        foreach (var (_, value, row) in _rows)
        {
            value.Password = "";
            RowsPanel.Children.Remove(row);
        }
        _rows.Clear();
        AddRow();
        HideError();
    }

    public void ShowError(string message)
    {
        // Closed and opened again, so Narrator reads a changed message (InfoBar page, learn.microsoft.com, read 2026-10-23).
        ErrorBar.IsOpen = false;
        ErrorBar.Message = message;
        ErrorBar.IsOpen = true;
    }

    public void HideError() => ErrorBar.IsOpen = false;

    /// <summary>Focus on the first name.</summary>
    public void FocusFirst()
    {
        if (_rows.Count > 0)
        {
            _rows[0].Name.Focus(FocusState.Programmatic);
        }
    }

    private void OnAdd(object sender, RoutedEventArgs e)
    {
        AddRow();
        _rows[^1].Name.Focus(FocusState.Programmatic);
    }

    private void OnUse(object sender, RoutedEventArgs e) => UseRequested?.Invoke(this, EventArgs.Empty);

    private void AddRow()
    {
        var number = _rows.Count + 1;
        var row = new Grid { ColumnSpacing = 8 };
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        var name = new TextBox
        {
            Header = "Name",
            IsSpellCheckEnabled = false,
            IsTextPredictionEnabled = false,
        };
        var value = new PasswordBox { Header = "Value" };
        var remove = new Button
        {
            Content = new SymbolIcon(Symbol.Delete),
            VerticalAlignment = VerticalAlignment.Bottom,
        };
        ToolTipService.SetToolTip(remove, "Remove");
        Grid.SetColumn(value, 1);
        Grid.SetColumn(remove, 2);
        row.Children.Add(name);
        row.Children.Add(value);
        row.Children.Add(remove);
        remove.Click += (_, _) => RemoveRow(row);
        _rows.Add((name, value, row));
        Label(_rows[^1], number);
        RowsPanel.Children.Add(row);
    }

    /// <summary>A list of "Name", "Value" and delete buttons would not tell its rows apart for Narrator: each is named by its row.</summary>
    private static void Label((TextBox Name, PasswordBox Value, Grid Row) row, int number)
    {
        AutomationProperties.SetName(row.Name, $"Header {number} name");
        AutomationProperties.SetName(row.Value, $"Header {number} value");
        AutomationProperties.SetName(row.Row.Children[2], $"Remove header {number}");
    }

    private void RemoveRow(Grid row)
    {
        var index = _rows.FindIndex(r => r.Row == row);
        if (index < 0)
        {
            return;
        }
        _rows[index].Value.Password = "";
        _rows.RemoveAt(index);
        RowsPanel.Children.Remove(row);
        if (_rows.Count == 0)
        {
            AddRow();
        }
        for (var i = 0; i < _rows.Count; i++)
        {
            Label(_rows[i], i + 1);
        }
        // Focus stays in the list: on the row now in this place, else the last one.
        _rows[Math.Min(index, _rows.Count - 1)].Name.Focus(FocusState.Programmatic);
    }
}
