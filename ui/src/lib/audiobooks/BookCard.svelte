<script lang="ts">
	/** stream: audiobooks — a book as a kit Card: opens the book page, plays (resumes) from the button. */
	import Card from '$lib/kit/Card.svelte';
	import { isPlaying, toggle } from '$lib/discover/play.svelte';
	import { bookHref, playBookById, type Audiobook } from '$lib/audiobooks.svelte';

	let { book }: { book: Pick<Audiobook, 'id' | 'title' | 'author' | 'thumbnail'> } = $props();
</script>

<Card
	title={book.title}
	subtitle={book.author}
	image={book.thumbnail ?? undefined}
	kind="audiobook"
	href={bookHref(book.id)}
	playing={isPlaying(book.id, book.title)}
	onPlay={() => toggle(book.id, () => playBookById(book.id), book.title)}
/>
