#set page(
  paper: "a4",
  margin: (left: 22mm, right: 22mm, top: 22mm, bottom: 20mm),
  header: context {
    if counter(page).get().first() > 1 {
      grid(
        columns: (1fr, auto),
        [#text(size: 8pt, fill: rgb("#68716b"))[GAINSNAP · USER MANUAL]],
        [#text(size: 8pt, fill: rgb("#68716b"))[v$version$]],
      )
      line(length: 100%, stroke: 0.5pt + rgb("#d6d8d2"))
    }
  },
  footer: context {
    if counter(page).get().first() > 1 {
      align(center, text(size: 8pt, fill: rgb("#68716b"))[#counter(page).display("1")])
    }
  },
)
#set document(date: none)
#set text(font: "New Computer Modern", size: 10pt, fill: rgb("#202523"), lang: "en")
#set par(justify: false, leading: 0.72em, spacing: 0.68em)
#set heading(numbering: "1.")
#show link: it => {
  set text(fill: rgb("#b74437"))
  underline(it)
}
#show heading.where(level: 1): it => block(above: 1.3em, below: 0.48em)[
  #set text(size: 21pt, weight: "bold", fill: rgb("#df5d4c"))
  #it
]
#show heading.where(level: 2): it => block(above: 1.05em, below: 0.35em)[
  #set text(size: 14pt, weight: "bold", fill: rgb("#202523"))
  #it
]
#show heading.where(level: 3): it => block(above: 0.85em, below: 0.25em)[
  #set text(size: 11pt, weight: "bold", fill: rgb("#df5d4c"))
  #it
]

#align(left)[
  #text(size: 29pt, weight: "bold", fill: rgb("#df5d4c"))[$title$]
  #v(0.35em)
  #text(size: 11pt, fill: rgb("#606963"))[$subtitle$]
  #v(0.25em)
  #text(size: 9pt, fill: rgb("#606963"))[$author$]
]
#v(0.55em)
#line(length: 100%, stroke: 1.2pt + rgb("#202523"))
#v(1.3em)
#outline(title: [Contents], depth: 2)
#pagebreak()

$body$
