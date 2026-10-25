using System.Diagnostics;
using Current.Bindings;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;

namespace Current.Shell.Browsing;

/// <summary>
/// One person of a detail page's cast and crew: the platform's PersonPicture, the name and the role. Bound to a <see cref="Person"/>
/// through its DataContext, as a tile is; it names itself for Narrator ("name, role").
/// </summary>
public sealed partial class PersonCard : UserControl
{
    private readonly TileImage _loader = new(App.Core!);

    public PersonCard()
    {
        InitializeComponent();
        DataContextChanged += (_, _) => Bind();
        // Before it is in a window there is no scale to size the picture by.
        Loaded += (_, _) => Bind();
        Unloaded += (_, _) => _loader.Recycle();
    }

    private void Bind()
    {
        _loader.Recycle();
        Picture.ProfilePicture = null;
        if (DataContext is not Person person)
        {
            return;
        }
        Picture.DisplayName = person.Name;
        NameText.Text = person.Name;
        RoleText.Text = person.Role is { Length: > 0 } role ? role : BrowseText.PersonKind(person.Kind);
        AutomationProperties.SetName(this, RoleText.Text.Length > 0 ? $"{person.Name}, {RoleText.Text}" : person.Name);
        if (person.Image is { } art && XamlRoot is { } root)
        {
            var size = (int)Math.Ceiling(Picture.Width * root.RasterizationScale);
            _ = LoadAsync(new ImageRequest(new ImageSource.Server(art.Image), size, size), person);
        }
    }

    private async Task LoadAsync(ImageRequest request, Person person)
    {
        try
        {
            var bitmap = await _loader.LoadAsync(request);
            if (bitmap is not null && ReferenceEquals(DataContext, person))
            {
                Picture.ProfilePicture = bitmap;
            }
        }
        catch (ImageLoadException)
        {
            // The initials stay.
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: a person's picture failed: {error.GetType().Name}");
        }
    }
}
