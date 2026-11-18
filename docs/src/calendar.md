# Patina Community Calendar

<style>
  .content main { max-width: 1200px; }
</style>

<div
  data-ics-url="https://outlook.office365.com/owa/calendar/925880b62a0e4026863de1b98875814c@microsoft.com/0651ec1d2ff045cda0c5e09ac3593d5712495824753958737181/calendar.ics"
  data-height="900px"
  data-views="month,week,agenda"
  data-weekstart="0"
></div>
<a data-ics-attribution href="https://icsviewer.com/">Calendar by ICS Viewer</a>

<script>
  // Match the calendar to the active mdBook theme. The calendar theme is updated when the page is loaded.
  (function () {
    var themes = {
      light: { bg: "ffffff", color: "2b79a2" },
      rust: { bg: "e1e1e1", color: "2b79a2" },
      coal: { bg: "141617", color: "4da6ff" },
      navy: { bg: "161923", color: "4da6ff" },
      ayu: { bg: "0f1419", color: "ffb454" },
    };
    var cls = document.documentElement.classList;
    var name = Object.keys(themes).find(function (t) { return cls.contains(t); }) || "light";
    var calendar = document.querySelector("[data-ics-url]");
    calendar.setAttribute("data-bg", themes[name].bg);
    calendar.setAttribute("data-color", themes[name].color);
  })();
</script>
<script src="https://icsviewer.com/widget.js" async></script>

[Open the Patina Community Calendar in full-page](https://icsviewer.com/view?url=https://outlook.office365.com/owa/calendar/925880b62a0e4026863de1b98875814c@microsoft.com/0651ec1d2ff045cda0c5e09ac3593d5712495824753958737181/calendar.ics)

Direct Teams call links:

- [Patina Teams Call (aka.ms/patina-teams-call)](https://aka.ms/patina-teams-call)
- [Patina Teams Call Asian Time Zones (aka.ms/patina-teams-call-asia)](https://aka.ms/patina-teams-call-asia)

## Subscribe to the Calendar

Subscribe to the calendar feed so meetings and updates appear in your own calendar automatically. Use this URL:

```text
https://outlook.office365.com/owa/calendar/925880b62a0e4026863de1b98875814c@microsoft.com/0651ec1d2ff045cda0c5e09ac3593d5712495824753958737181/calendar.ics
```

It is recommended to subscribe to the feed instead of importing the file. An import is a one-time copy while
subscribe will keep your calendar up to date with any changes.

- **Outlook on the web / new Outlook**: Calendar > Add calendar > Subscribe from web. Paste the URL and select Import.
- **Outlook (classic desktop)**: File > Account Settings > Account Settings > Internet Calendars > New. Paste the URL.
- **Google Calendar**: Other calendars (+) > From URL. Paste the URL and select Add calendar.
- **Apple Calendar**: File > New Calendar Subscription. Paste the URL.

Note that subscribed calendars are refreshed by your calendar application on its own schedule. Some applications may
several hours or longer to reflect updates.
