
## lyrics

the lyrics should be accessible (if they exist ofc) thru the cli, something like a `:lyrics` command works. the command works on any tab, q closes the lyrics panel. the panel is scrollable thru j/k as always. panel remains on whatever screen it was called on. say you call it on `[3] artists` then switch back to `[1] queue`, you won't see the lyrics panel, youll see the default queue view. toast pops up if you call `:lyrics` but none are detected for that particular song. lyrics panel stays up if youre listening to a song with the panel up and then the song moves to the next song in the queue. the panel will stay up even if the next song doesnt have lyrics and you get the same toast and the lyrics panel appears empty with the text "no lyrics detected". the player controls are all still fully accessible with the lyrics panel up.

## search

search doesnt work right as it is. say you search "adrianne lenker", then hit enter on the search result, it just adds all of adrianne's songs to your queue. this obv isn't how a search function should work. a song should be added to the queue only if the search result itself was a song and then the user hit play on it. in any other case, say the search result is an album, user clicks enter on it and it takes the user back to `[2] albums` with the search result album selected, same with the artists search results and hell even playlists when we add those.

## playlist

unsure for the interface for interacting (making, adding songs, deleting, browsing playlists) with playlists since the current ui doesnt scale well, its kinda just limited to the existing 4 nav tabs but people who make playlists and listen to songs thru playlists tend to have many many playlists. open to ideas on this

## last minor tweak

currently song and album titles can get cut off, see attached screenshot. song and album title and song length should never get cut off. if a song title or whatever is too long and is getting cut off it should become like a scrolling ribbon so all the deets are still visible
