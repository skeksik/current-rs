using BridgeSpike.ViewModels;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using uniffi.bridge_spike;

namespace BridgeSpike;

public sealed partial class MainWindow : Window
{
    private readonly Core _core;

    public MainWindow()
    {
        // The first call starts the core's runtime. A failure here is a BridgeException.Runtime.
        _core = new Core();
        var ui = DispatcherQueue;
        Value = new ValueViewModel(_core, ui);
        Items = new ItemsViewModel(_core, ui);
        Operation = new OperationViewModel(_core, ui);
        InitializeComponent();
        Closed += OnClosed;
    }

    public ValueViewModel Value { get; }
    public ItemsViewModel Items { get; }
    public OperationViewModel Operation { get; }

    private void OnSubscribedToggled(object sender, RoutedEventArgs e)
    {
        if (((ToggleSwitch)sender).IsOn) Value.Subscribe();
        else Value.Unsubscribe();
    }

    // The window is the "view going away": every handle is cancelled here, not left to the GC.
    private void OnClosed(object sender, WindowEventArgs args)
    {
        Operation.Dispose();
        Items.Dispose();
        Value.Dispose();
        _core.Dispose();
    }
}
