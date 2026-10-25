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

    /// <summary>The person shown now: bound again to the same one (Loaded after DataContextChanged), the card keeps its photo.</summary>
    private Person? _shown;

    private void Bind()
    {
        if (DataContext is not Person person)
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
        // The initials only once the photo is late, as a tile's BlurHash (TileImage.PlaceholderDelay): a photo the core has shows
        // without initials before it (the owner's run, 2026-10-25).
        Picture.DisplayName = "";
        NameText.Text = person.Name;
        RoleText.Text = person.Role is { Length: > 0 } role ? role : BrowseText.PersonKind(person.Kind);
        AutomationProperties.SetName(this, RoleText.Text.Length > 0 ? $"{person.Name}, {RoleText.Text}" : person.Name);
        if (person.Image is not { } art)
        {
            Picture.DisplayName = person.Name;
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
        _ = ShowInitialsLaterAsync(person);
    }

    private async Task ShowInitialsLaterAsync(Person person)
    {
        await Task.Delay(TileImage.PlaceholderDelay);
        if (ReferenceEquals(_shown, person) && Picture.ProfilePicture is null)
        {
            Picture.DisplayName = person.Name;
        }
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
            // No photo: the initials, at once.
            if (ReferenceEquals(_shown, person))
            {
                Picture.DisplayName = person.Name;
            }
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: a person's picture failed: {error.GetType().Name}");
        }
    }
}
