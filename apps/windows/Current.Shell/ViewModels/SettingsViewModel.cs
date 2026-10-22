using System.ComponentModel;
using Current.Bindings;
using Microsoft.UI.Xaml;

namespace Current.Shell.ViewModels;

/// <summary>What the settings page shows: the core's version and its capabilities state, mirrored on the UI thread.</summary>
public sealed class SettingsViewModel(CoreHost? core, IUiDispatcher ui) : INotifyPropertyChanged
{
    private ValueSubscription<Capabilities>? _capabilities;

    public event PropertyChangedEventHandler? PropertyChanged;

    public string VersionText { get; } = core is null ? "" : $"Version {core.AppInfo.Version}";

    public IReadOnlyList<ServerStatus> Servers => _capabilities?.Value.Servers ?? [];

    public Visibility NoServersVisibility => Servers.Count == 0 ? Visibility.Visible : Visibility.Collapsed;

    public string DiagnosticsText() => core?.DiagnosticsText() ?? "";

    public void Start()
    {
        if (core is null || _capabilities is not null)
        {
            return;
        }
        _capabilities = core.ObserveCapabilities(ui);
        _capabilities.PropertyChanged += OnCapabilitiesChanged;
        Changed();
    }

    public void Stop()
    {
        if (_capabilities is null)
        {
            return;
        }
        _capabilities.PropertyChanged -= OnCapabilitiesChanged;
        _capabilities.Dispose();
        _capabilities = null;
    }

    private void OnCapabilitiesChanged(object? sender, PropertyChangedEventArgs e) => Changed();

    private void Changed()
    {
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(Servers)));
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(NoServersVisibility)));
    }
}
