// 轻量下拉框：原生 <select> 在 macOS / Windows 上样式不可控，这里自己画一个。
//
// 用法：
//   const sel = UISelect.create(el, { onChange(value) {} })
//   sel.setItems([{ value: 'a', label: 'A' }], 'a')
//   sel.setValue('a')
;(function () {
  const CHEVRON =
    '<svg viewBox="0 0 24 24" width="14" height="14" aria-hidden="true"><path d="m6 9 6 6 6-6" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>'

  function create(container, options) {
    const opts = options || {}
    const button = document.createElement('button')
    button.type = 'button'
    button.className = 'select-button'
    button.setAttribute('aria-haspopup', 'listbox')
    button.setAttribute('aria-expanded', 'false')

    const label = document.createElement('span')
    label.className = 'select-label'
    const arrow = document.createElement('span')
    arrow.className = 'select-arrow'
    arrow.innerHTML = CHEVRON
    button.append(label, arrow)

    container.classList.add('select')
    container.innerHTML = ''
    container.appendChild(button)

    let items = []
    let value = opts.value || ''
    let popup = null

    function current() {
      return items.find((item) => item.value === value)
    }

    function render() {
      const found = current()
      label.textContent = found ? found.label : opts.placeholder || ''
      button.title = found ? found.label : ''
    }

    function close() {
      if (!popup) return
      popup.remove()
      popup = null
      button.setAttribute('aria-expanded', 'false')
      container.classList.remove('is-open')
      document.removeEventListener('mousedown', onOutside, true)
      document.removeEventListener('keydown', onKey, true)
    }

    function onOutside(event) {
      if (popup && !popup.contains(event.target) && !button.contains(event.target)) close()
    }

    function onKey(event) {
      if (!popup) return
      if (event.key === 'Escape') {
        event.preventDefault()
        close()
        button.focus()
        return
      }
      if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return
      event.preventDefault()
      const rows = [...popup.querySelectorAll('.select-item')]
      const index = rows.indexOf(document.activeElement)
      const next = event.key === 'ArrowDown' ? Math.min(index + 1, rows.length - 1) : Math.max(index - 1, 0)
      if (rows[next]) rows[next].focus()
    }

    function open() {
      if (popup) {
        close()
        return
      }
      popup = document.createElement('div')
      popup.className = 'select-popup'
      popup.setAttribute('role', 'listbox')
      for (const item of items) {
        const row = document.createElement('button')
        row.type = 'button'
        row.className = 'select-item' + (item.value === value ? ' is-selected' : '')
        row.setAttribute('role', 'option')
        row.setAttribute('aria-selected', String(item.value === value))
        row.textContent = item.label
        row.addEventListener('click', () => {
          value = item.value
          render()
          close()
          if (opts.onChange) opts.onChange(value)
        })
        popup.appendChild(row)
      }
      document.body.appendChild(popup)

      // 贴到按钮下方；空间不够就翻到上方
      const rect = button.getBoundingClientRect()
      popup.style.left = `${Math.round(rect.left)}px`
      popup.style.minWidth = `${Math.round(rect.width)}px`
      const height = popup.offsetHeight
      const below = window.innerHeight - rect.bottom
      if (height + 12 > below && rect.top > height + 12) {
        popup.style.top = `${Math.round(rect.top - height - 6)}px`
        popup.classList.add('is-up')
      } else {
        popup.style.top = `${Math.round(rect.bottom + 6)}px`
      }
      button.setAttribute('aria-expanded', 'true')
      container.classList.add('is-open')
      document.addEventListener('mousedown', onOutside, true)
      document.addEventListener('keydown', onKey, true)
      const selected = popup.querySelector('.is-selected') || popup.querySelector('.select-item')
      if (selected) selected.focus()
    }

    button.addEventListener('click', open)
    button.addEventListener('keydown', (event) => {
      if (event.key === 'ArrowDown' || event.key === 'Enter' || event.key === ' ') {
        event.preventDefault()
        open()
      }
    })

    render()

    return {
      get value() {
        return value
      },
      setItems(next, nextValue) {
        items = next || []
        if (nextValue !== undefined) value = nextValue
        render()
      },
      setValue(next) {
        value = next
        render()
      },
    }
  }

  window.UISelect = { create }
})()
