using System.Diagnostics;
using Current.Bindings;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;

namespace Current.Shell.Browsing;

/// <summary>
/// One person of a detail page's cast and crew: the platform's PersonPicture, the name and the role. Bound to a <see cref="Person"/>
/// through its DataContext, as a tile is; it names itself for Narrator ("name, role"). A search's People bind it to a tile's slot, a
/// person's item summary: its name and picture, no role.
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

    /// <summary>
    /// The person shown now. Bound again to the same one (Loaded after DataContextChanged, or a DataContextChanged WinUI raises with
    /// nothing changed, as StableSource describes), the card keeps its photo instead of going back to the initials.
    /// </summary>
    private Person? _shown;

    private void Bind()
    {
        var person = DataContext switch
        {
            Person cast => cast,
            GridSlot { Summary: { } summary } => new Person(
                summary.Item, summary.Name, null, "", summary.Artwork.FirstOrDefault(art => art.Kind == "primary")),
            _ => null,
        };
        if (person is null)
        {
            _loader.Recycle();
            _shown = null;
            Picture.ProfilePicture = null;
            Picture.DisplayName = "";
            return;
        }
        if (person == _shown && (Picture.ProfilePicture is not null || XamlRoot is null))
        {
            return;
        }
        _loader.Recycle();
        Picture.ProfilePicture = null;
        // The initials until the photo arrives, or for good without one.
        Picture.DisplayName = person.Name;
        NameText.Text = person.Name;
        RoleText.Text = person.Role is { Length: > 0 } role ? role : BrowseLabels.PersonKind(person.Kind);
        AutomationProperties.SetName(this, RoleText.Text.Length > 0 ? $"{person.Name}, {RoleText.Text}" : person.Name);
        if (person.Image is not { } art)
        {
            _shown = person;
            return;
        }
        if (XamlRoot is not { } root)
        {
            return;
        }
        _shown = person;
        var size = (int)Math.Ceiling(Picture.Width * root.RasterizationScale);
        _ = LoadAsync(new ImageRequest(new ImageSource.Server(art.Image), size, size), person);
    }

    private async Task LoadAsync(ImageRequest request, Person person)
    {
        try
        {
            var bitmap = await _loader.LoadAsync(request);
            if (bitmap is not null && ReferenceEquals(_shown, person))
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
